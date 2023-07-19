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
    BroadcastUpdate,
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

// used for stuwe parser/message generator
#[derive(Serialize, Deserialize, Debug)]
pub struct MealsForDay {
    pub date: String,
    pub meal_groups: Vec<MealGroup>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MealGroup {
    pub meal_type: String,
    pub sub_meals: Vec<SingleMeal>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SingleMeal {
    pub name: String,
    pub additional_ingredients: Vec<String>,
    pub price: String,
}
