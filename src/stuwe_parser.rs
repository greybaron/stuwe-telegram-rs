use crate::data_types::MealGroup;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use rand::Rng;
use teloxide::utils::markdown;

use std::{collections::HashMap, time::Instant};

async fn get_mensimates_json(date: &str, mensa: &str) -> Option<String> {
    let mut map = HashMap::new();

    map.insert("apiUsername", "apiuser_telegram");
    map.insert("password", "telegrambesser");

    let client = reqwest::Client::new();
    let login_resp = client
        .post("https://api.olech2412.de/mensaHub/auth/login")
        .json(&map)
        .send()
        .await;

    match login_resp {
        Ok(login_resp) => {
            let token = login_resp.text().await.unwrap();

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
        Err(_) => None,
    }

    // response.text().await.unwrap()
}

pub async fn build_meal_message(days_forward: i64, mensa_location: u8) -> String {
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
    let day_meals = get_meals(requested_date, mensa_location).await;
    let f_date = german_date_fmt(requested_date.date_naive());

    // warn if requested "today" was raised to next monday (requested on sat/sun)
    let future_day_info = if days_forward == 0 && date_raised_by_days == 1 {
        Some(" (Morgen)")
    } else if days_forward == 0 && date_raised_by_days == 2 {
        Some(" (Übermorgen)")
    } else {
        None
    };

    let emojis = ["☀️", "🦀", "💂🏻‍♀️", "☕️", "🍽️", "☝🏻", "🌤️"];
    let rand_emoji = emojis[rand::thread_rng().gen_range(0..emojis.len())];
    msg += &format!(
        "{} {}{} {}\n",
        rand_emoji,
        markdown::italic(&f_date),
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

                    let first_price = meal_group.first().unwrap().price.clone();
                    let price_is_shared = meal_group.iter().all(|item| item.price == first_price);

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
                        msg += &format!("  {}\n", first_price);
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

async fn get_meals(requested_date: DateTime<Local>, mensa_location: u8) -> Option<Vec<MealGroup>> {
    // returns meals struct either from cache,
    // or starts html request, parses data; returns data and also triggers saving to cache

    // let (date, mensa) = build_url_params(requested_date, mensa_location);
    let mm_json = mm_json_request(requested_date, mensa_location).await;

    match mm_json {
        Some(text) => {
            // let mut file = File::create("mm_json.json").await.unwrap();
            // file.write_all(text.as_bytes()).await.unwrap();

            let day_meal_groups: Vec<MealGroup> = serde_json::from_str(&text).unwrap();
            Some(day_meal_groups)
        }
        None => None,
    }
    // // write mm_json to file

    // serde::Deserialize::deserialize(&mm_json).unwrap();
    // let downloaded_meals = extract_data_from_html(&mm_json, day).await;
    // serialize downloaded meals
    // let downloadked_json_text = serde_json::to_string(&downloaded_meals).unwrap();´
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

fn german_date_fmt(date: NaiveDate) -> String {
    let mut output = String::new();

    let week_days = [
        "Montag, ",
        "Dienstag, ",
        "Mittwoch, ",
        "Donnerstag, ",
        "Freitag, ",
    ];

    output += week_days[date.weekday().num_days_from_monday() as usize];
    output += &date.format("%d.%m.%Y").to_string();

    output
}

fn escape_markdown_v2(input: &str) -> String {
    // all 'special' chars have to be escaped when using telegram markdown_v2

    input
        .replace('.', r"\.")
        .replace('!', r"\!")
        .replace('+', r"\+")
        .replace('-', r"\-")
        .replace('<', r"\<")
        .replace('>', r"\>")
        .replace('(', r"\(")
        .replace(')', r"\)")
        .replace('=', r"\=")
        // workaround as '&' in html is improperly decoded
        .replace("&amp;", "&")
}

// pub async fn update_cache(mensen: &Vec<u8>) -> Vec<u8> {
//     // will be run periodically: requests all possible dates (heute/morgen/ueb) and creates/updates caches
//     // returns a vector of mensa locations whose 'today' plan was updated

//     // days will be selected using this rule:
//     // if current day ... then ...

//     //     Thu =>
//     //         'heute' => thursday
//     //         'morgen' => friday
//     //         'uebermorgen' => monday

//     //     Fri =>
//     //         'heute' => friday
//     //         'morgen'/'uebermorgen' => monday

//     //     Sat =>
//     //         'heute'/'morgen'/'uebermorgen' => monday

//     //     Sun =>
//     //         'heute'/'morgen' => monday
//     //         'uebermorgen' => tuesday

//     //     Mon/Tue/Wed => as you'd expect

//     let mut days: Vec<DateTime<Local>> = Vec::new();

//     // get at most 3 days from (inclusive) today, according to the rule above
//     let today = chrono::Local::now();

//     for i in 0..3 {
//         let day = today + Duration::days(i);

//         if ![Weekday::Sat, Weekday::Sun].contains(&day.weekday()) {
//             days.push(day);
//         // weekend day is not first day (i>0) but within 3 day range, so add monday & break
//         } else if i != 0 {
//             if day.weekday() == Weekday::Sat {
//                 days.push(day + Duration::days(2));
//             } else {
//                 days.push(day + Duration::days(1));
//             }
//             break;
//         }
//     }

//     // add task handles to vec so that they can be awaited after spawing
//     let mut handles = Vec::new();
//     let mut mensen_today_changed = Vec::new();

//     for mensa_id in mensen {
//         // spawning task for every day
//         for day in &days {
//             handles.push(tokio::spawn({
//                 mm_json_request(*day, *mensa_id)
//             }))
//         }
//     }

//     // let mut today_changed = false;
//     // awaiting results of every task
//     for handle in handles {
//         if let Some(mensa_id) = handle.await.unwrap().0 {
//             mensen_today_changed.push(mensa_id);
//         }
//     }

//     mensen_today_changed
// }

// async fn meals_to_json() -> String {

// }

// async fn json_cache_to_meal(url_params: &str) -> Option<MealsForDay> {
//     match File::open(format!("cached_data/{}.json", &url_params)).await {
//         // cached file exists, use that
//         Ok(mut file) => {
//             let now = Instant::now();
//             let mut json_text = String::new();
//             file.read_to_string(&mut json_text).await.unwrap();

//             let day_meals: MealsForDay = serde_json::from_str(&json_text).unwrap();
//             log::debug!("json cache deser: {:.2?}", now.elapsed());

//             Some(day_meals)
//         }
//         // no cached file, use reqwest
//         Err(_) => None,
//     }
// }

async fn mm_json_request(day: DateTime<Local>, mensa_id: u8) -> Option<String> {
    let (date, mensa) = build_url_params(day, mensa_id);

    // getting data from server
    let req_now = Instant::now();
    // let downloaded_html = reqwest_get_html_text(&url_params).await;
    let mm_json = get_mensimates_json(&date, &mensa).await;
    log::debug!("got MM json after {:.2?}", req_now.elapsed());

    mm_json

    // read (and update) cached json
    //  match File::open(format!("cached_data/{}.json", &url_params)).await {
    //     Ok(mut json) => {
    //         let mut cache_json_text = String::new();
    //         json.read_to_string(&mut cache_json_text).await.unwrap();

    //         // cache was updated, return mensa_id if 'today' was updated
    //         // return new data if requested
    //         if downloaded_json_text != cache_json_text {
    //             save_update_cache(&url_params, &downloaded_json_text).await;
    //             (if day.weekday() == chrono::Local::now().weekday() {Some(mensa_id)} else {None},
    //             Some(downloaded_meals))
    //         } else {
    //             (None, Some(downloaded_meals))
    //         }
    //     }
    //     Err(_) => {
    //         save_update_cache(&url_params, &downloaded_json_text).await;
    //         (
    //             if day.weekday() == chrono::Local::now().weekday() {Some(mensa_id)} else {None},
    //             Some(downloaded_meals)
    //         )
    //     }
    // }
}
