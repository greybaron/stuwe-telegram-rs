use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct MealGroup {
    pub meal_type: String,
    pub sub_meals: Vec<SingleMeal>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SingleMeal {
    pub name: String,
    pub additional_ingredients: Vec<String>,
    pub allergens: Option<String>,
    pub variations: Option<Vec<MealVariation>>,
    pub price: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct MealVariation {
    pub name: String,
    pub allergens_and_add: Option<String>,
}
