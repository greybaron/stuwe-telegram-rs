use serde::{Deserialize, Serialize};

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
    pub rating: f32,
    // servingDate: String,
    // starsTotal: i64,
    pub votes: i64,
}
