use axum::{
    extract::{State, Path},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, FromRow};
use thiserror::Error;
use uuid::Uuid;
use bcrypt::{hash, verify, DEFAULT_COST};
mod config; 

#[derive(Debug, Error)]
enum ApiError {
    #[error("Student not found")]
    NotFound,
    #[error("Invalid credentials")]
    Unauthorized,
    #[error("Username already exists")]
    Conflict,
    #[error("Bad request: {0}")]
    BadRequest(String),
    #[error("Internal server error")]
    InternalServerError
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Conflict => StatusCode::CONFLICT,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::InternalServerError => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()    
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateStudentRequest {
    username: String,
    password: String,
    name: String,
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
struct StudentResponse {
    username: String,
    name: String,
    matric_number: Option<String>
}

#[derive(Debug, Serialize, Deserialize)]
struct LoginRequest {
    username: String,
    password: String
}

#[derive(Debug, FromRow)]
struct Student {
    id: Uuid,
    username: String,
    password: String,
    name: String,
    matric_number: Option<String>
}

impl Student {
    fn to_response(&self) -> StudentResponse {  
        StudentResponse {
            username: self.username.clone(),
            name: self.name.clone(),
            matric_number: self.matric_number.clone()
        }
    }
}

#[derive(Debug, Clone)]
struct AppState {
    pool: PgPool
}

async fn create_student(
    State(state): State<AppState>, 
    Json(payload): Json<CreateStudentRequest>) 
    -> Result<Json<StudentResponse>, ApiError> {
    let hashed_password = hash(&payload.password, DEFAULT_COST)
        .map_err(|e| ApiError::BadRequest(format!("Password hashing failed: {}", e)))?;

    let student = sqlx::query_as!(
        Student,
        r#"
        INSERT INTO students (username, password, name) 
        VALUES ($1, $2, $3) 
        RETURNING id, username, password, name, matric_number  -- Fixed: added password
        "#,
        payload.username,
        hashed_password,
        payload.name
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(err) if err.constraint() == Some("students_username_key") => {
            ApiError::Conflict
        }
        _ => ApiError::InternalServerError
    })?;

    Ok(Json(student.to_response()))
}

async fn assign_matric_number(
    State(state): State<AppState>,
    Path(username): Path<String> 
) -> Result<Json<StudentResponse>, ApiError> {
    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM students WHERE matric_number IS NOT NULL"
    ).fetch_one(&state.pool).await.map_err(|_| ApiError::InternalServerError)?;
    
    let matric_number = format!("MAT{:05}", count.unwrap_or(0) + 1);

    let student = sqlx::query_as!(
        Student, 
        r#"
        UPDATE students
        SET matric_number = $1
        WHERE username = $2 AND matric_number IS NULL 
        RETURNING id, username, password, name, matric_number
        "#, 
        matric_number, 
        username
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| ApiError::InternalServerError)?
    .ok_or_else(|| {
        ApiError::BadRequest("Student not found or already has matric number".to_string())
    })?;

    Ok(Json(student.to_response()))
}

async fn list_students(State(state): State<AppState>) -> Result<Json<Vec<StudentResponse>>, ApiError> {  // Fixed: Vec<StudentResponse>
    let students = sqlx::query_as!(
        Student,
        "SELECT id, username, password, name, matric_number FROM students"
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|_| ApiError::InternalServerError)?;

    let student_responses: Vec<StudentResponse> = students
        .into_iter()
        .map(|student| student.to_response())
        .collect();
        
    Ok(Json(student_responses))
}

async fn get_student_by_matric(
    State(state): State<AppState>, 
    Path(matric_number): Path<String> 
) -> Result<Json<StudentResponse>, ApiError> {
    let student = sqlx::query_as!(
        Student, 
        r#"
        SELECT id, username, password, name, matric_number 
        FROM students 
        WHERE matric_number = $1
        "#, 
        matric_number
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| ApiError::InternalServerError)?
    .ok_or(ApiError::NotFound)?;
    
    Ok(Json(student.to_response()))
}

async fn login(
    State(state): State<AppState>, 
    Json(payload): Json<LoginRequest>
) -> Result<Json<StudentResponse>, ApiError> {
    let student = sqlx::query_as!(
        Student, 
        r#"
        SELECT id, username, password, name, matric_number FROM students  -- Fixed: password typo
        WHERE username = $1
        "#, 
        payload.username
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| ApiError::InternalServerError)?
    .ok_or(ApiError::Unauthorized)?;

    if !verify(&payload.password, &student.password).map_err(|_| ApiError::Unauthorized)? {
        return Err(ApiError::Unauthorized);
    }

    Ok(Json(student.to_response()))
}
async fn hello() -> String {
    return "hello World".to_string();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::Config::load();
    println!("DB URL: {}", config.db_url);
    
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.db_url)
        .await?;

    sqlx::migrate!().run(&pool).await?;
    let app_state = AppState { pool };

    let app = Router::new()
        .route("/", get(hello))
        .route("/students", post(create_student))
        .route("/students/{username}/matric", post(assign_matric_number))
        .route("/students", get(list_students))
        .route("/students/matric/{matric_number}", get(get_student_by_matric))
        .route("/login", post(login))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("Server running on http://localhost:3000");
    axum::serve(listener, app).await?;
    Ok(())
}