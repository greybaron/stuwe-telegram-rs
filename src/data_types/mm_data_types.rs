use serde::{Deserialize, Serialize};

// #[derive(Serialize, Deserialize, Debug, Clone)]
// pub struct MealGroup {
//     pub id: u64,
//     pub name: String,
//     pub description: String,
//     pub price: String,
//     pub category: String,
//     #[serde(rename = "servingDate")]
//     pub serving_date: String,
//     #[serde(rename = "additionalInfo")]
//     pub additional_info: String,
//     pub allergens: String,
//     pub additives: String,
//     pub rating: f32,
//     pub votes: u64,
// }

#[derive(Serialize, Deserialize, Debug)]
pub struct GetMensasMensa {
    pub id: u8,
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MensiMeal {
    // allergens: String,
    pub category: String,
    pub description: String,
    // id: i64,
    pub name: String,
    pub price: String,
    // rating: f64,
    // servingDate: String,
    // starsTotal: i64,
    // votes: i64,
}
