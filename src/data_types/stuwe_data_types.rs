use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MealGroup {
    pub meal_type: String,
    pub sub_meals: Vec<SingleMeal>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CanteenMealsDay {
    pub canteen_id: u32,
    pub meal_groups: Vec<MealGroup>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CanteenMealDiff {
    pub canteen_id: u32,
    pub new_meals: Option<Vec<MealGroup>>,
    pub modified_meals: Option<Vec<MealGroup>>,
    pub removed_meals: Option<Vec<MealGroup>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SingleMeal {
    pub name: String,
    pub additional_ingredients: Vec<String>,
    pub allergens: Option<String>,
    pub variations: Option<Vec<MealVariation>>,
    pub price: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MealVariation {
    pub name: String,
    pub allergens_and_add: Option<String>,
}
