use crate::data_types::{MealGroup, MealsForDay, nMealGroup, SingleMeal};

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use scraper::{Html, Selector};
use selectors::{attr::CaseSensitivity, Element};
use teloxide::utils::markdown;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt}; // for file operations

use std::{collections::HashMap, time::Instant};

async fn get_mensimates_json(date: &str, mensa: &str) -> String {
    let mut map = HashMap::new();

    map.insert("apiUsername", "apiuser_telegram");
    map.insert("password", "telegrambesser");

    let client = reqwest::Client::new();
    let login_resp = client
        .post("https://api.olech2412.de/mensaHub/auth/login")
        .json(&map)
        .send()
        .await
        .unwrap();

    let token = login_resp.text().await.unwrap();

    let now = Instant::now();

    let response = client
        .get(format!("https://api.olech2412.de/mensaHub/{}/servingDate/{}", mensa, date))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    let txt = response.text().await.unwrap();
    // write string to file
    
    txt
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
    log::debug!("determine request date: {:.2?}", now.elapsed());

    // retrieve meals
    let day_meals = get_meals(requested_date, mensa_location).await;
    let f_date = german_date_fmt(requested_date.date_naive());


    // start message formatting
    let now = Instant::now();

    // warn if requested "today" was raised to next monday (requested on sat/sun)
    let future_day_info = if days_forward == 0 && date_raised_by_days == 1 {
        Some(" (Morgen)")
    } else if days_forward == 0 && date_raised_by_days == 2 {
        Some(" (Übermorgen)")
    } else {
        None
    };

    // insert date + potential future day warning
    msg += &format!(
        "{}{}\n",
        markdown::italic(&f_date),
        future_day_info.unwrap_or_default()
    );

    if day_meals.is_empty() {
        msg += &markdown::bold("\nkeine Daten vorhanden.\n");
    }

    // loop over meal groups
    for meal_group in day_meals {
        // let mut price_is_shared = true;
        // let price_first_submeal = &meal_group.sub_meals.first().unwrap().price;

        // for sub_meal in &meal_group.sub_meals {
        //     if &sub_meal.price != price_first_submeal {
        //         price_is_shared = false;
        //         break;
        //     }
        // }

        // Bold type of meal (-group)
        let sub_ingredients = &meal_group.description.split('&').map(|x| x.trim()).collect::<Vec<&str>>();
        msg += &format!("\n{}\n", markdown::bold(&meal_group.category));
        msg += &format!(" • {}\n", markdown::underline(&meal_group.name));
        for ingr in sub_ingredients {
            msg += &format!("     + {}\n", markdown::italic(ingr));
        }
        msg += &format!("   {}\n", &meal_group.price);
        
        // loop over meals in meal group
        // for sub_meal in &meal_group.sub_meals {
        //     // underlined single or multiple meal name
        //     msg += &format!(" • {}\n", markdown::underline(&sub_meal.name));

        //     // loop over ingredients of meal
        //     for ingredient in &sub_meal.additional_ingredients {
        //         // appending ingredient to msg
        //         msg += &format!("     + {}\n", markdown::italic(ingredient))
        //     }
        //     // appending price

        //     msg += &format!("   {}\n", sub_meal.price);

        // }
    }

    msg += "\n < /heute >  < /morgen >\n < /uebermorgen >";

    // return
    escape_markdown_v2(&msg)
}

async fn get_meals(requested_date: DateTime<Local>, mensa_location: u8) -> Vec<nMealGroup> {
    // returns meals struct either from cache,
    // or starts html request, parses data; returns data and also triggers saving to cache

    // let (date, mensa) = build_url_params(requested_date, mensa_location);
    let mm_json = mm_json_request(requested_date, mensa_location).await.unwrap();

    // // // // write mm_json to file
    // // let mut file = File::create("mm_json.json").await.unwrap();
    // // file.write_all(mm_json.as_bytes()).await.unwrap();

    let day_meal_groups: Vec<nMealGroup> = serde_json::from_str(&mm_json).unwrap();

    // serde::Deserialize::deserialize(&mm_json).unwrap();
    // let downloaded_meals = extract_data_from_html(&mm_json, day).await;
    // serialize downloaded meals
    // let downloadked_json_text = serde_json::to_string(&downloaded_meals).unwrap();´
    
    day_meal_groups
}

async fn reqwest_get_html_text(url_params: &String) -> String {
    // requests html from server for chosen url params and returns html_text string

    let url_base: String =
        "https://www.studentenwerk-leipzig.de/mensen-cafeterien/speiseplan?".to_string();

    reqwest::get(url_base + url_params)
        .await
        .expect("URL request failed")
        .text()
        .await
        .unwrap()
}

async fn extract_data_from_html(html_text: &str, requested_date: DateTime<Local>) -> MealsForDay {
    let now = Instant::now();
    let mut v_meal_groups: Vec<MealGroup> = Vec::new();

    let document = Html::parse_fragment(html_text);

    // retrieving reported date and comparing to requested date
    let date_sel = Selector::parse(r#"select#edit-date>option[selected='selected']"#).unwrap();
    let received_date_human = document.select(&date_sel).next().unwrap().inner_html();

    let sub = received_date_human.split(' ').nth(1).unwrap();
    let received_date = NaiveDate::parse_from_str(sub, "%d.%m.%Y").unwrap();

    if received_date != requested_date.naive_local().date() {
        return MealsForDay {
            date: german_date_fmt(requested_date.naive_local().date()),
            meal_groups: v_meal_groups,
        };
    }

    let container_sel = Selector::parse(r#"section.meals"#).unwrap();
    let all_child_select = Selector::parse(r#":scope > *"#).unwrap();

    let container = document.select(&container_sel).next().unwrap();

    for child in container.select(&all_child_select) {
        if child
            .value()
            .has_class("title-prim", CaseSensitivity::CaseSensitive)
        {
            // title-prim == new group -> init new group struct
            let mut meals_in_group: Vec<SingleMeal> = Vec::new();

            let mut next_sibling = child.next_sibling_element().unwrap();

            // skip headlines (or other junk elements)
            // might loop infinitely (or probably crash) if last element is not of class .accordion.ublock :)
            while !(next_sibling
                .value()
                .has_class("accordion", CaseSensitivity::CaseSensitive)
                && next_sibling
                    .value()
                    .has_class("u-block", CaseSensitivity::CaseSensitive))
            {
                next_sibling = next_sibling.next_sibling_element().unwrap();
            }

            // "next_sibling" is now of class ".accordion.u-block", aka. a group of 1 or more dishes
            // -> looping over meals in group
            for dish_element in next_sibling.select(&all_child_select) {
                let mut additional_ingredients: Vec<String> = Vec::new();

                // looping over meal ingredients
                for add_ingred_element in
                    dish_element.select(&Selector::parse(r#"details>ul>li"#).unwrap())
                {
                    additional_ingredients.push(add_ingred_element.inner_html());
                }

                // collecting into SingleMeal struct
                let meal = SingleMeal {
                    name: dish_element
                        .select(&Selector::parse(r#"header>div>div>h4"#).unwrap())
                        .next()
                        .unwrap()
                        .inner_html(),
                    additional_ingredients, //
                    price: dish_element
                        .select(&Selector::parse(r#"header>div>div>p"#).unwrap())
                        .next()
                        .unwrap()
                        .inner_html()
                        .split('\n')
                        .last()
                        .unwrap()
                        .trim()
                        .to_string(),
                };

                // pushing SingleMeal to meals struct
                meals_in_group.push(meal);
            }

            // collecting into MealGroup struct
            let meal_group = MealGroup {
                meal_type: child.inner_html(),
                sub_meals: meals_in_group,
            };

            // pushing MealGroup to MealGroups struct
            v_meal_groups.push(meal_group);
        }
    }

    log::debug!("parsing html: {:.2?}", now.elapsed());
    MealsForDay {
        date: received_date_human,
        meal_groups: v_meal_groups,
    }
}

async fn save_update_cache(url_params: &str, json_text: &str) {
    // check cache dir existence, and create if not found
    std::fs::create_dir_all("cached_data/").expect("failed to create data cache dir");

    let mut json_file = File::create(format!("cached_data/{}.json", &url_params))
        .await
        .expect("failed to create json cache file");
    json_file
        .write_all(json_text.as_bytes())
        .await
        .expect("failed to write to a json file");
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

async fn mm_json_request(
    day: DateTime<Local>,
    mensa_id: u8,
) -> Option<String> {
    let (date, mensa) = build_url_params(day, mensa_id);
    
    // getting data from server
    let req_now = Instant::now();
    // let downloaded_html = reqwest_get_html_text(&url_params).await;
    let mm_json = get_mensimates_json(&date, &mensa).await;
    log::debug!("got MM json after {:.2?}", req_now.elapsed());

    Some(mm_json)

    
    

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
