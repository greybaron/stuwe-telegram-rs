use std::collections::BTreeMap;

use crate::constants::API_URL;
use crate::data_backend::{escape_markdown_v2, german_date_fmt, EMOJIS};
use crate::data_types::stuwe_data_types::MealGroup;

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, Weekday};
use rand::Rng;
use teloxide::utils::markdown;

use tokio::time::Instant;
pub async fn stuwe_build_meal_msg(days_forward: i64, mensa_location: u8) -> String {
    // get requested date
    let mut requested_date = chrono::Local::now() + Duration::days(days_forward);
    let mut date_raised_by_days = 0;

    match requested_date.weekday() {
        // sat -> change req_date to mon
        Weekday::Sat => {
            requested_date += Duration::days(2);
            date_raised_by_days = 2;
        }
        Weekday::Sun => {
            requested_date += Duration::days(1);
            date_raised_by_days = 1;
        }
        _ => {
            // Any other weekday is fine, nothing to do
        }
    }

    // retrieve meals
    let now = Instant::now();
    match get_meals_from_api(requested_date, mensa_location).await {
        Err(_) => "Ein Fehler ist aufgetreten.".to_string(),
        Ok(meal_groups) => {
            log::debug!("API data: {:.2?}", now.elapsed());
            mealgroups_to_msg(
                meal_groups,
                requested_date,
                days_forward,
                date_raised_by_days,
            )
        }
    }
}

fn mealgroups_to_msg(
    meal_groups: Vec<MealGroup>,
    requested_date: DateTime<Local>,
    days_forward: i64,
    date_raised_by_days: i32,
) -> String {
    let mut msg: String = String::new();
    // start message formatting
    let rand_emoji = EMOJIS[rand::thread_rng().gen_range(0..EMOJIS.len())];
    msg += &format!(
        "{} {} {}\n",
        rand_emoji,
        german_date_fmt(requested_date.date_naive()),
        rand_emoji,
    );

    // warn if requested "today" was raised to next monday (requested on sat/sun)
    if days_forward == 0 && date_raised_by_days == 1 {
        msg += &markdown::italic("      (Morgen)\n");
    } else if days_forward == 0 && date_raised_by_days == 2 {
        msg += &markdown::italic("      (Übermorgen)\n")
    }

    if meal_groups.is_empty() {
        msg += &markdown::bold("\nkeine Daten vorhanden.\n");
    }

    // loop over meal groups
    for meal_group in meal_groups {
        let price_first_meal = meal_group.sub_meals.first().unwrap().price.clone();
        let price_is_shared = meal_group
            .sub_meals
            .iter()
            .all(|item| item.price == price_first_meal);

        // Bold type of meal (-group)
        msg += &format!("\n{}\n", markdown::bold(&meal_group.meal_type));

        // loop over meals in meal group
        for sub_meal in &meal_group.sub_meals {
            // underlined single or multiple meal name
            msg += &format!(" • {}\n", markdown::underline(&sub_meal.name));

            // loop over ingredients of meal
            for ingredient in &sub_meal.additional_ingredients {
                // appending ingredient to msg
                msg += &format!("     + {}\n", markdown::italic(ingredient))
            }
            if let Some(allergens) = sub_meal.allergens.as_ref() {
                msg += &format!("    ℹ️ {}\n", allergens)
            }
            // appending price
            if !price_is_shared {
                msg += &format!("   {}\n", sub_meal.price);
            }

            if let Some(variations) = sub_meal.variations.as_ref() {
                msg += &format!("   → {}\n", markdown::bold("Variationen:"));
                for variation in variations {
                    msg += &format!("       • {}\n", markdown::italic(&variation.name));
                    if let Some(allergens_and_add) = variation.allergens_and_add.as_ref() {
                        msg += &format!("         ℹ️ {}\n", allergens_and_add)
                    }
                }
            }
        }
        if price_is_shared {
            msg += &format!("  {}\n", price_first_meal);
        }
    }

    escape_markdown_v2(&msg)
}

async fn get_meals_from_api(requested_date: DateTime<Local>, mensa: u8) -> Result<Vec<MealGroup>> {
    let date_str = build_date_string(requested_date);
    let client = reqwest::Client::new();
    let meal_groups = client
        .get(format!(
            "{}/canteens/{}/days/{}",
            API_URL.get().unwrap(),
            mensa,
            date_str
        ))
        .send()
        .await?
        .json()
        .await?;

    Ok(meal_groups)
}

fn build_date_string(requested_date: DateTime<Local>) -> String {
    let (year, month, day) = (
        requested_date.year(),
        requested_date.month(),
        requested_date.day(),
    );

    format!("{:04}-{:02}-{:02}", year, month, day)
}

pub async fn get_mensen() -> Result<BTreeMap<u8, String>> {
    let client = reqwest::Client::new();

    #[derive(serde::Deserialize)]
    struct Mensa {
        id: u8,
        name: String,
    }

    let mensa_list = client
        .get(format!("{}/canteens", API_URL.get().unwrap()))
        .send()
        .await?
        .json::<Vec<Mensa>>()
        .await?;

    Ok(mensa_list.into_iter().map(|m| (m.id, m.name)).collect())
}
