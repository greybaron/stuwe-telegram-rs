use serde::{Deserialize, Serialize};

// used for stuwe parser/message generator
#[derive(Serialize, Deserialize, Debug)]
pub struct DataForMensaForDay {
    pub mensa_id: u8,
    pub meal_groups: Vec<MealGroup>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MealGroup {
    pub meal_type: String,
    pub sub_meals: Vec<SingleMeal>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SingleMeal {
    pub name: String,
    pub additional_ingredients: Vec<String>,
    pub price: String,
}
