use crate::data_backend::{escape_markdown_v2, german_date_fmt, EMOJIS};
use crate::data_types::mm_data_types::MealGroup;
use chrono::{DateTime, Datelike, Duration, Local, Weekday};
use rand::Rng;
use teloxide::utils::markdown;
use tokio::sync::RwLock;

use anyhow::{Context, Result};
use std::sync::Arc;
use std::{collections::HashMap, time::Instant};

pub async fn get_jwt_token() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(55))
        .build()?;

    let json = HashMap::from([
        ("apiUsername", "apiuser_telegram"),
        ("password", "telegrambesser"),
    ]);

    client
        .post("https://api.olech2412.de/mensaHub/auth/login")
        .json(&json)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
        .context("JWT request returned no text")
}

async fn mm_json_request(
    day: DateTime<Local>,
    mensa_id: u8,
    jwt_lock: Arc<RwLock<String>>,
) -> Result<String, reqwest::Error> {
    let (date, mensa) = build_url_params(day, mensa_id);

    let req_now = Instant::now();
    let mm_json = get_mensimates_json(&mensa, &date, jwt_lock).await;
    log::debug!("got MM data after {:.2?}", req_now.elapsed());

    mm_json
}

fn build_url_params(requested_date: DateTime<Local>, mensa_location: u8) -> (String, String) {
    let (year, month, day) = (
        requested_date.year(),
        requested_date.month(),
        requested_date.day(),
    );

    let mensen: HashMap<u8, &str> = HashMap::from([
        (153, "cafeteria_dittrichring"),
        (127, "menseria_am_botanischen_garten"),
        (118, "mensa_academica"),
        (106, "mensa_am_park"),
        (115, "mensa_am_elsterbecken"),
        (162, "mensa_am_medizincampus"),
        (111, "mensa_peterssteinweg"),
        (140, "mensa_schoenauerstr"),
        (170, "mensa_tierklinik"),
    ]);

    let date = format!("{:04}-{:02}-{:02}", year, month, day);
    let mensa = mensen.get(&mensa_location).unwrap().to_string();

    (date, mensa)
}

async fn get_mensimates_json(
    mensa: &str,
    date: &str,
    jwt_lock: Arc<RwLock<String>>,
) -> Result<String, reqwest::Error> {
    let client = reqwest::Client::new();

    let token = jwt_lock.read().await.clone();

    let response = client
        .get(format!(
            "https://api.olech2412.de/mensaHub/{}/servingDate/{}",
            mensa, date
        ))
        .bearer_auth(&token)
        .send()
        .await?
        .error_for_status()?;

    response.text().await
}

pub async fn mm_build_meal_msg(
    days_forward: i64,
    mensa_location: u8,
    jwt_lock: Arc<RwLock<String>>,
) -> String {
    let mut msg: String = String::new();

    // all nows & .elapsed() are for performance metrics
    let now = Instant::now();

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
    log::debug!("build req params: {:.2?}", now.elapsed());

    // retrieve meals
    let day_meals = get_meals_from_db(requested_date, mensa_location, jwt_lock).await;
    let german_date = german_date_fmt(requested_date.date_naive());

    // start message formatting
    let rand_emoji = EMOJIS[rand::thread_rng().gen_range(0..EMOJIS.len())];
    msg += &format!(
        "{} {} {}\n",
        rand_emoji,
        markdown::italic(&german_date),
        rand_emoji,
    );

    // warn if requested "today" was raised to next monday (requested on sat/sun)
    if days_forward == 0 && date_raised_by_days == 1 {
        msg += &markdown::italic("      (Morgen)\n");
    } else if days_forward == 0 && date_raised_by_days == 2 {
        msg += &markdown::italic("      (Übermorgen)\n")
    }

    match day_meals {
        Ok(meals) => {
            if meals.is_empty() {
                msg += &markdown::bold("\nkeine Daten vorhanden.\n");
            } else {
                let mut structured_day_meals: HashMap<String, Vec<MealGroup>> = HashMap::new();

                // loop over meal groups
                for meal in meals {
                    if let Some(cat) = structured_day_meals.get_mut(&meal.category) {
                        cat.push(meal);
                    } else {
                        structured_day_meals.insert(meal.category.clone(), vec![meal]);
                    }
                }

                for meal_group in structured_day_meals {
                    // Bold type of meal (-group)
                    let title = meal_group.0;
                    msg += &format!("\n{}\n", markdown::bold(&title));

                    let meals_in_group = meal_group.1;
                    if meals_in_group.is_empty() {
                        msg += &format!(" • {}\n", markdown::underline("Keine Gerichte..."));
                        continue;
                    }

                    let price_first_meal = meals_in_group.first().unwrap().price.clone();
                    let price_is_shared = meals_in_group
                        .iter()
                        .all(|item| item.price == price_first_meal);

                    for meal in meals_in_group {
                        msg += &format!(" • {}\n", markdown::underline(&meal.name));
                        let sub_ingredients = &meal
                            .description
                            .split('&')
                            .map(|x| x.trim())
                            .collect::<Vec<&str>>();
                        for ingr in sub_ingredients {
                            if *ingr != "N/A" && !ingr.is_empty() {
                                msg += &format!("     + {}\n", markdown::italic(ingr));
                            }
                        }
                        if !price_is_shared {
                            msg += &format!("  {}\n", &meal.price);
                        }
                    }

                    if price_is_shared {
                        msg += &format!("   {}\n", price_first_meal);
                    }
                }
            }
        }
        Err(e) => {
            msg += &markdown::bold("\nMensiMates-Abfrage fehlgeschlagen:\n");
            msg += &e.to_string().replace('_', r"\_");
        }
    }

    escape_markdown_v2(&msg)
}

async fn get_meals_from_db(
    requested_date: DateTime<Local>,
    mensa_location: u8,
    jwt_lock: Arc<RwLock<String>>,
) -> Result<Vec<MealGroup>, reqwest::Error> {
    let mm_json = mm_json_request(requested_date, mensa_location, jwt_lock).await;

    match mm_json {
        Ok(text) => {
            let day_meal_groups: Vec<MealGroup> = serde_json::from_str(&text).unwrap();
            Ok(day_meal_groups)
        }
        Err(e) => Err(e),
    }
}
