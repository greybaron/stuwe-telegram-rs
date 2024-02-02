use std::collections::BTreeMap;
use std::vec;

use crate::data_backend::{escape_markdown_v2, german_date_fmt, EMOJIS};
use crate::data_types::stuwe_data_types::{DataForMensaForDay, MealGroup, SingleMeal};
use crate::data_types::STUWE_DB;
use crate::db_operations::mensa_name_get_id_db;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use rand::Rng;
use rusqlite::{params, Connection};
use scraper::{ElementRef, Html, Selector};
use selectors::Element;
use teloxide::utils::markdown;

use tokio::time::Instant;
pub async fn stuwe_build_meal_msg(days_forward: i64, mensa_location: u8) -> String {
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
    let day_meals = get_meals_from_db(requested_date, mensa_location).await;

    // start message formatting
    let rand_emoji = EMOJIS[rand::thread_rng().gen_range(0..EMOJIS.len())];
    msg += &format!(
        "{} {} {}\n",
        rand_emoji,
        german_date_fmt(requested_date.date_naive()),
        // markdown::italic(&day_meals.date),
        rand_emoji,
    );

    // warn if requested "today" was raised to next monday (requested on sat/sun)
    if days_forward == 0 && date_raised_by_days == 1 {
        msg += &markdown::italic("      (Morgen)\n");
    } else if days_forward == 0 && date_raised_by_days == 2 {
        msg += &markdown::italic("      (Übermorgen)\n")
    }

    if day_meals.is_empty() {
        msg += &markdown::bold("\nkeine Daten vorhanden.\n");
    }

    // loop over meal groups
    for meal_group in day_meals {
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

async fn get_meals_from_db(requested_date: DateTime<Local>, mensa: u8) -> Vec<MealGroup> {
    let date_str = build_date_string(requested_date);
    let json_text = get_meal_from_db(&date_str, mensa).await;
    if let Some(json_text) = json_text {
        json_to_meal(&json_text).await
    } else {
        vec![]
    }
}

async fn reqwest_get_html_text(date: &str) -> Result<String> {
    let url_base =
        "https://www.studentenwerk-leipzig.de/mensen-cafeterien/speiseplan?date=".to_string();

    Ok(reqwest::get(url_base + date).await?.text().await?)
}

async fn extract_data_from_html(
    html_text: &str,
    requested_date: DateTime<Local>,
) -> Result<Vec<DataForMensaForDay>> {
    let mut all_data_for_day = vec![];

    let now = Instant::now();
    let document = Html::parse_fragment(html_text);

    let date_button_group_sel = Selector::parse(r#"button.date-button.is--active"#).unwrap();
    let active_date_button = document
        .select(&date_button_group_sel)
        .next()
        .context("Recv. StuWe site is invalid (has no date)")?;

    let received_date_str = active_date_button.attr("data-date").unwrap().to_owned();
    let received_date = NaiveDate::parse_from_str(&received_date_str, "%Y-%m-%d")
        .context("unexpected StuWe date format")?;

    // if received date != requested date -> return empty meals struct (isn't an error, just StuWe being weird)
    if received_date != requested_date.date_naive() {
        return Ok(vec![]);
    }

    let container_sel = Selector::parse(r#"div.meal-overview"#).unwrap();

    let meal_containers: Vec<ElementRef> = document.select(&container_sel).collect();
    // .collect()
    // .context("StuWe site has no meal container")?;
    if meal_containers.is_empty() {
        return Err(anyhow!("StuWe site has no meal containers"));
    }

    for meal_container in meal_containers {
        if let Some(mensa_title_element) = meal_container.prev_sibling_element() {
            let mensa_title = mensa_title_element.inner_html();
            let meals = extract_mealgroup_from_htmlcontainer(meal_container)?;
            if let Some(mensa_id) = mensa_name_get_id_db(STUWE_DB, &mensa_title) {
                all_data_for_day.push(DataForMensaForDay {
                    mensa_id,
                    meal_groups: meals,
                });
            } else {
                log::warn!(target: "stuwe_telegram_rs::stuwe_parser", "Mensa not found in DB: {}", mensa_title);
            }
        }
    }

    log::debug!(target: "stuwe_telegram_rs::stuwe_parser", "HTML → Data: {:.2?}", now.elapsed());
    Ok(all_data_for_day)
}

fn extract_mealgroup_from_htmlcontainer(meal_container: ElementRef<'_>) -> Result<Vec<MealGroup>> {
    let mut v_meal_groups: Vec<MealGroup> = Vec::new();

    let meal_sel = Selector::parse(r#"div.type--meal"#).unwrap();
    let meal_type_sel = Selector::parse(r#"div.meal-tags>.tag"#).unwrap();
    let title_sel = Selector::parse(r#"h4"#).unwrap();
    let additional_ingredients_sel = Selector::parse(r#"div.meal-components"#).unwrap();
    let price_sel = Selector::parse(r#"div.meal-prices>span"#).unwrap();

    // quick && dirty
    for meal_element in meal_container.select(&meal_sel) {
        let meal_type = meal_element
            .select(&meal_type_sel)
            .next()
            .context("meal category element not found")?
            .inner_html();

        let title = meal_element
            .select(&title_sel)
            .next()
            .context("meal title element not found")?
            .inner_html();

        let additional_ingredients =
            if let Some(item) = meal_element.select(&additional_ingredients_sel).next() {
                let text = item.inner_html();
                // for whatever reason there might be, sometimes this element exists without any content
                if !text.is_empty() {
                    item.inner_html()
                        .split('·')
                        .map(|slice| slice.trim().to_string())
                        .collect()
                } else {
                    // in that case, return empty vec (otherwise it would be a vec with one empty string in it)
                    vec![]
                }
                // Sosumi
            } else {
                vec![]
            };

        let mut price = String::new();
        meal_element.select(&price_sel).for_each(|price_element| {
            price += &price_element.inner_html();
        });

        // oh my
        // oh my
        match v_meal_groups
            .iter_mut()
            .find(|meal_group| meal_group.meal_type == meal_type)
        {
            None => {
                // doesn't exist yet, create new meal group of new meal type
                v_meal_groups.push(MealGroup {
                    meal_type,
                    sub_meals: vec![SingleMeal {
                        name: title,
                        price,
                        additional_ingredients,
                    }],
                });
            }
            Some(meal_group) => {
                // meal group of this type already exists, add meal to it
                meal_group.sub_meals.push(SingleMeal {
                    name: title,
                    price,
                    additional_ingredients,
                });
            }
        }
    }

    Ok(v_meal_groups)
}

async fn save_meal_to_db(date: &str, mensa: u8, json_text: &str) {
    let conn = Connection::open(STUWE_DB).unwrap();
    conn.execute(
        "delete from meals where mensa_id = ?1 and date = ?2",
        [mensa.to_string(), date.to_string()],
    )
    .unwrap();

    let mut stmt = conn
        .prepare_cached(
            "insert into meals (mensa_id, date, json_text)
            values (?1, ?2, ?3)",
        )
        .unwrap();

    stmt.execute(params![mensa, date, json_text]).unwrap();
}

async fn get_meal_from_db(date: &str, mensa: u8) -> Option<String> {
    let conn = Connection::open(STUWE_DB).unwrap();
    let mut stmt = conn
        .prepare_cached("select json_text from meals where (mensa_id, date) = (?1, ?2)")
        .unwrap();
    let mut rows = stmt.query([&mensa.to_string(), date]).unwrap();

    rows.next().unwrap().map(|row| row.get(0).unwrap())
}

fn build_date_string(requested_date: DateTime<Local>) -> String {
    let (year, month, day) = (
        requested_date.year(),
        requested_date.month(),
        requested_date.day(),
    );

    format!("{:04}-{:02}-{:02}", year, month, day)
}

pub async fn update_cache() -> Result<Vec<u8>> {
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

    for day in &days {
        handles.push(tokio::spawn(parse_and_save_meals(*day)))
    }

    // awaiting results of every task
    for handle in handles {
        let mut changed_mensen_ids = handle.await??;
        mensen_today_changed.append(&mut changed_mensen_ids);
    }

    log::debug!(
        "{} Mensen changed meals of current day",
        mensen_today_changed.len()
    );

    Ok(mensen_today_changed)
}

async fn json_to_meal(json_text: &str) -> Vec<MealGroup> {
    serde_json::from_str(json_text).unwrap()
}

async fn parse_and_save_meals(day: DateTime<Local>) -> Result<Vec<u8>> {
    let mut today_changed_mensen_ids = vec![];

    let date_string = build_date_string(day);

    // getting data from server
    let downloaded_html = reqwest_get_html_text(&date_string).await?;

    let all_data_for_day = extract_data_from_html(&downloaded_html, day).await?;
    // serialize downloaded meals
    for mensa_data_for_day in all_data_for_day {
        let downloaded_json_text = serde_json::to_string(&mensa_data_for_day.meal_groups).unwrap();
        let db_json_text = get_meal_from_db(&date_string, mensa_data_for_day.mensa_id)
            .await
            .unwrap_or_default();

        // if downloaded meals are different from cached meals, update cache
        if downloaded_json_text != db_json_text {
            log::debug!(
                "updating cache: Mensa={} Date={}",
                mensa_data_for_day.mensa_id,
                date_string
            );
            save_meal_to_db(
                &date_string,
                mensa_data_for_day.mensa_id,
                &downloaded_json_text,
            )
            .await;

            if day.weekday() == chrono::Local::now().weekday() {
                today_changed_mensen_ids.push(mensa_data_for_day.mensa_id);
            }
        }
    }

    Ok(today_changed_mensen_ids)
}

pub async fn get_mensen() -> BTreeMap<u8, String> {
    let mut mensen = BTreeMap::new();

    // pass invalid date to get empty page (dont need actual data) with all mensa locations
    let html_text = reqwest_get_html_text("a").await.unwrap_or_default();
    let document = Html::parse_fragment(&html_text);
    let mensa_list_sel = Selector::parse(r#"#content-header>.dropdown-pane>li"#).unwrap();
    let mensa_item_sel = Selector::parse(r#"a"#).unwrap();
    for header in document.select(&mensa_list_sel) {
        if let Some(mensa_id) = header.value().attr("data-location") {
            if let Some(mensa_name) = header.select(&mensa_item_sel).next() {
                mensen.insert(mensa_id.parse::<u8>().unwrap(), mensa_name.inner_html());
            }
        }
    }

    if mensen.is_empty() {
        log::error!("Failed to load mensen from stuwe, falling back");
        BTreeMap::from(
            [
                (153, "Cafeteria Dittrichring"),
                (127, "Mensaria am Botanischen Garten"),
                (118, "Mensa Academica"),
                (106, "Mensa am Park"),
                (115, "Mensa am Elsterbecken"),
                (162, "Mensa am Medizincampus"),
                (111, "Mensa Peterssteinweg"),
                (140, "Mensa Schönauer Straße"),
                (170, "Mensa Tierklinik"),
            ]
            .map(|(id, name)| (id, name.to_string())),
        )
    } else {
        mensen
    }
}
