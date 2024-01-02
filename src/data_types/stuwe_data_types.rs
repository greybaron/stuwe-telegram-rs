use serde::{Deserialize, Serialize};
use thiserror::Error;

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
#[derive(Clone)]
pub struct CampusDualData {
    pub username: String,
    pub password: String,
    pub chat_id: i64,
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CampusDualGrade {
    pub name: String,
    pub grade: String,
    pub subgrades: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CampusDualSignupOption {
    pub name: String,
    pub verfahren: String,
    pub status: String,
}

#[derive(Error, Debug, Clone)]
pub enum CampusDualError {
    #[error("CampusDual init failed: {0}")]
    CdInitFailed(u16),
    #[error("CampusDual zba_init failed: {0}")]
    CdZbaFailed(u16),
    #[error("CampusDual: bad credentials")]
    CdBadCredentials,
}
