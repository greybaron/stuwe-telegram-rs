use crate::data_backend::{escape_markdown_v2, german_date_fmt, EMOJIS};
use crate::data_types::stuwe_data_types::{MealGroup, MealsForDay, SingleMeal};

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use rand::Rng;
use rusqlite::{params, Connection};
use scraper::{Html, Selector};
use selectors::{attr::CaseSensitivity, Element};
use teloxide::utils::markdown;

use tokio::time::Instant;
pub async fn stuwe_b_meal_msg(days_forward: i64, mensa_location: u8) -> String {
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

    // start message formatting
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
        markdown::italic(&day_meals.date),
        future_day_info.unwrap_or_default(),
        rand_emoji
    );

    if day_meals.meal_groups.is_empty() {
        msg += &markdown::bold("\nkeine Daten vorhanden.\n");
    }

    // loop over meal groups
    for meal_group in day_meals.meal_groups {
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
            // appending price
            if !price_is_shared {
                msg += &format!("   {}\n", sub_meal.price);
            }
        }
        if price_is_shared {
            msg += &format!("   {}\n", price_first_meal);
        }
    }

    escape_markdown_v2(&msg)
}

async fn get_meals(requested_date: DateTime<Local>, mensa_location: u8) -> MealsForDay {
    // returns meals struct either from cache,
    // or starts html request, parses data; returns data and also triggers saving to cache

    let url_params = build_url_params(requested_date, mensa_location);

    let json_text = get_meal_from_db(&url_params).await;
    json_to_meal(&json_text.unwrap()).await.unwrap()
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

    log::debug!(target: "stuwe_telegram_rs::stuwe_parser", "HTML → Data: {:.2?}", now.elapsed());
    MealsForDay {
        date: received_date_human,
        meal_groups: v_meal_groups,
    }
}

async fn save_meal_to_db(url_params: &str, json_text: &str) {
    let conn = Connection::open("storage.sqlite").unwrap();
    let mut stmt = conn
        .prepare_cached(
            "replace into meals (mensa_and_date, json_text)
            values (?1, ?2)",
        )
        .unwrap();

    stmt.execute(params![url_params, json_text,]).unwrap();
}

async fn get_meal_from_db(url_params: &str) -> Option<String> {
    let conn = Connection::open("storage.sqlite").unwrap();
    let mut stmt = conn
        .prepare_cached("select json_text from meals where mensa_and_date = (?1)")
        .unwrap();
    let mut rows = stmt.query([url_params]).unwrap();

    rows.next().unwrap().map(|row| row.get(0).unwrap())
}

fn build_url_params(requested_date: DateTime<Local>, mensa_location: u8) -> String {
    let (year, month, day) = (
        requested_date.year(),
        requested_date.month(),
        requested_date.day(),
    );

    format!(
        "location={}&date={:04}-{:02}-{:02}",
        mensa_location, year, month, day
    )
}

pub async fn update_cache(mensen: &Vec<u8>) -> Vec<u8> {
    // will be run periodically: requests all possible dates (heute/morgen/ueb) and creates/updates caches
    // returns a vector of mensa locations whose 'today' plan was updated

    // days will be selected using this rule:
    // if current day ... then ...

    //     Thu =>
    //         'heute' => thursday
    //         'morgen' => friday
    //         'uebermorgen' => monday

    //     Fri =>
    //         'heute' => friday
    //         'morgen'/'uebermorgen' => monday

    //     Sat =>
    //         'heute'/'morgen'/'uebermorgen' => monday

    //     Sun =>
    //         'heute'/'morgen' => monday
    //         'uebermorgen' => tuesday

    //     Mon/Tue/Wed => as you'd expect

    let mut days: Vec<DateTime<Local>> = Vec::new();

    // get at most 3 days from (inclusive) today, according to the rule above
    let today = chrono::Local::now();

    for i in 0..3 {
        let day = today + Duration::days(i);

        if ![Weekday::Sat, Weekday::Sun].contains(&day.weekday()) {
            days.push(day);
        // weekend day is not first day (i>0) but within 3 day range, so add monday & break
        } else if i != 0 {
            if day.weekday() == Weekday::Sat {
                days.push(day + Duration::days(2));
            } else {
                days.push(day + Duration::days(1));
            }
            break;
        }
    }

    // add task handles to vec so that they can be awaited after spawing
    let mut handles = Vec::new();
    let mut mensen_today_changed = Vec::new();

    for mensa_id in mensen {
        // spawning task for every day
        for day in &days {
            handles.push(tokio::spawn(save_to_cache(*day, *mensa_id)))
        }
    }

    // let mut today_changed = false;
    // awaiting results of every task
    for handle in handles {
        if let Some(mensa_id) = handle.await.unwrap().0 {
            mensen_today_changed.push(mensa_id);
        }
    }

    mensen_today_changed
}

async fn json_to_meal(json_text: &str) -> Option<MealsForDay> {
    if !json_text.is_empty() {
        Some(serde_json::from_str(json_text).unwrap())
    } else {
        None
    }
}

async fn save_to_cache(day: DateTime<Local>, mensa_id: u8) -> (Option<u8>, Option<MealsForDay>) {
    let url_params = build_url_params(day, mensa_id);

    // getting data from server
    let downloaded_html = reqwest_get_html_text(&url_params).await;

    let downloaded_meals = extract_data_from_html(&downloaded_html, day).await;
    // serialize downloaded meals
    let downloaded_json_text = serde_json::to_string(&downloaded_meals).unwrap();
    let db_json_text = get_meal_from_db(&url_params).await.unwrap_or_default();

    // if downloaded meals are different from cached meals, update cache
    if downloaded_json_text != db_json_text {
        save_meal_to_db(&url_params, &downloaded_json_text).await;
        let today_updated = if day.weekday() == chrono::Local::now().weekday() {
            Some(mensa_id)
        } else {
            None
        };
        (today_updated, Some(downloaded_meals))
    } else {
        (None, Some(downloaded_meals))
    }
}
