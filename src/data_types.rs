use thiserror::Error;
use serde::{Deserialize, Serialize};

// used for teloxide/Telegram bot itself
#[derive(Debug, Copy, Clone)]
pub enum JobType {
    Register,
    Unregister,
    CheckJobExists
}

// pretty lazy
#[derive(Debug, Copy, Clone)]
pub struct JobHandlerTask {
    pub job_type: JobType,
    pub chatid: i64,
    pub hour: Option<u32>,
    pub minute: Option<u32>
}

#[derive(Debug, Error, Clone)]
pub enum TimeParseError {
    #[error("Keine Zeit übergeben")]
    NoTimePassed,
    #[error("Zeit konnte nicht gelesen werden")]
    InvalidTimePassed
}

#[derive(Serialize, Deserialize)]
pub struct DayMeals {
    pub date: String,
    pub meal_groups: Vec<MealGroup>,
}

#[derive(Serialize, Deserialize)]
pub struct MealGroup {
    pub meal_type: String,
    pub sub_meals: Vec<SingleMeal>,
}

#[derive(Serialize, Deserialize)]
pub struct SingleMeal {
    pub name: String,
    pub additional_ingredients: Vec<String>,
    pub price: String,
}