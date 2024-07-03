use std::sync::OnceLock;

use crate::data_types::Backend;

pub const NO_DB_MSG: &str = "Bitte zuerst /start ausführen";
pub const MENSI_DB: &str = "mensimates.sqlite";
pub const STUWE_DB: &str = "stuwe.sqlite";

pub static DB_FILENAME: OnceLock<&str> = OnceLock::new();
pub static BACKEND: OnceLock<Backend> = OnceLock::new();

pub static OLLAMA_HOST: OnceLock<Option<String>> = OnceLock::new();
pub static OLLAMA_MODEL: OnceLock<Option<String>> = OnceLock::new();
