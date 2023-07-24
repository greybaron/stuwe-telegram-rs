use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// used internally for teloxide/Telegram bot
#[derive(Debug, Clone)]
pub enum JobType {
    Register,
    Unregister,
    QueryRegistration,
    UpdateRegistration,
}

#[derive(Debug, Clone)]
pub struct JobHandlerTask {
    pub job_type: JobType,
    pub chat_id: Option<i64>,
    pub mensa_id: Option<u8>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
}

// opt (job uuid), mensa id, opt(hour), opt(min)
pub type RegistrationEntry = (Option<Uuid>, u8, Option<u32>, Option<u32>);

#[derive(Error, Debug, Clone)]
pub enum TimeParseError {
    #[error("Keine Zeit übergeben")]
    NoTimePassed,
    #[error("Zeit konnte nicht gelesen werden")]
    InvalidTimePassed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MealGroup {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub price: String,
    pub category: String,
    #[serde(rename = "servingDate")]
    pub serving_date: String,
    #[serde(rename = "additionalInfo")]
    pub additional_info: String,
    pub allergens: String,
    pub additives: String,
    pub rating: f32,
    pub votes: u64,
}
