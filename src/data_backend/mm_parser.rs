use crate::data_backend::{escape_markdown_v2, german_date_fmt, EMOJIS};
use crate::data_types::mm_data_types::MealGroup;

use chrono::{DateTime, Datelike, Duration, Local, Weekday};
use rand::Rng;
use teloxide::utils::markdown;
use tokio::sync::RwLock;

use std::sync::Arc;
use std::{collections::HashMap, time::Instant};

pub async fn get_jwt_token() -> String {
    let client = reqwest::Client::new();
    let mut map = HashMap::new();

    map.insert("apiUsername", "apiuser_telegram");
    map.insert("password", "telegrambesser");

    let login_resp = client
        .post("https://api.olech2412.de/mensaHub/auth/login")
        .json(&map)
        .send()
        .await;

    login_resp.unwrap().text().await.unwrap()
}

async fn mm_json_request(
    day: DateTime<Local>,
    mensa_id: u8,
    jwt_lock: Arc<RwLock<String>>,
) -> Option<String> {
    let (date, mensa) = build_url_params(day, mensa_id);

    let req_now = Instant::now();
    let mm_json = get_mensimates_json(&mensa, &date, jwt_lock).await;
    log::debug!("got MM json after {:.2?}", req_now.elapsed());

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
) -> Option<String> {
    let client = reqwest::Client::new();

    let token = jwt_lock.read().await.clone();

    let response = client
        .get(format!(
            "https://api.olech2412.de/mensaHub/{}/servingDate/{}",
            mensa, date
        ))
        .bearer_auth(&token)
        .send()
        .await;

    match response {
        Ok(t) => Some(t.text().await.unwrap()),
        Err(_) => None,
    }
}

pub async fn build_meal_message(
    days_forward: i64,
    mensa_location: u8,
    jwt_lock: Arc<RwLock<String>>,
) -> String {
    let mut msg: String = String::new();

    // all nows & .elapsed() are for performance info
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
    let day_meals = get_meals(requested_date, mensa_location, jwt_lock).await;
    let german_date = german_date_fmt(requested_date.date_naive());

    // warn if requested "today" was raised to next monday (requested on sat/sun)
    let future_day_info = if days_forward == 0 && date_raised_by_days == 1 {
        Some(" (Morgen)")
    } else if days_forward == 0 && date_raised_by_days == 2 {
        Some(" (Übermorgen)")
    } else {
        None
    };

    let rand_emoji = EMOJIS[rand::thread_rng().gen_range(0..EMOJIS.len())];
    msg += &format!(
        "{} {}{} {}\n",
        rand_emoji,
        markdown::italic(&german_date),
        future_day_info.unwrap_or_default(),
        rand_emoji
    );

    match day_meals {
        Some(meals) => {
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

                    let meal_group = meal_group.1;

                    let price_first_meal = meal_group.first().unwrap().price.clone();
                    let price_is_shared =
                        meal_group.iter().all(|item| item.price == price_first_meal);

                    for meal in meal_group {
                        msg += &format!(" • {}\n", markdown::underline(&meal.name));
                        let sub_ingredients = &meal
                            .description
                            .split('&')
                            .map(|x| x.trim())
                            .collect::<Vec<&str>>();
                        for ingr in sub_ingredients {
                            if *ingr != "N/A" {
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
        None => {
            msg += &markdown::bold("\nMensiMates-Abfrage fehlgeschlagen.\n");
        }
    }

    escape_markdown_v2(&msg)
}

async fn get_meals(
    requested_date: DateTime<Local>,
    mensa_location: u8,
    jwt_lock: Arc<RwLock<String>>,
) -> Option<Vec<MealGroup>> {
    let mm_json = mm_json_request(requested_date, mensa_location, jwt_lock).await;

    match mm_json {
        Some(text) => {
            let day_meal_groups: Vec<MealGroup> = serde_json::from_str(&text).unwrap();
            Some(day_meal_groups)
        }
        None => None,
    }
}
