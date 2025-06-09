pub struct Config {
    pub db_url: String,
}

impl Config {
    pub fn load() -> Self {
        if let Err(e) = dotenvy::dotenv() {
            eprintln!("⚠️ Couldn't load .env: {e}");
        }

        Config {
            db_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
        }
    }
}