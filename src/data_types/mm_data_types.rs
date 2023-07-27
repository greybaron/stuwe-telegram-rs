use serde::{Deserialize, Serialize};

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