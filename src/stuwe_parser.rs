use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Weekday};
use scraper::{Html, Selector};
use selectors::{attr::CaseSensitivity, Element};
use teloxide::utils::markdown;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt}; // for write_to_file etc

use crate::data_types::{DayMeals, MealGroup, SingleMeal};

pub async fn build_meal_message(days_forward: i64) -> String {
    #[cfg(feature = "benchmark")]
    let now = Instant::now();

    let mut msg: String = String::new();

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

    #[cfg(feature = "benchmark")]
    println!("req setup took: {:.2?}", now.elapsed());

    // retrieve meals
    let day_meals = get_meals(requested_date).await;

    // start message formatting
    #[cfg(feature = "benchmark")]
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
        markdown::italic(&day_meals.date),
        future_day_info.unwrap_or_default()
    );

    if day_meals.meal_groups.is_empty() {
        msg += &markdown::bold("\nkeine Daten vorhanden.\n");
    }

    // loop over meal groups
    for meal_group in day_meals.meal_groups {
        let mut price_is_shared = true;
        let price_first_submeal = &meal_group.sub_meals.first().unwrap().price;

        for sub_meal in &meal_group.sub_meals {
            if &sub_meal.price != price_first_submeal {
                price_is_shared = false;
                break;
            }
        }

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
            msg += &format!("   {}\n", price_first_submeal);
        }
    }

    msg += "\n < /heute >  < /morgen >\n < /uebermorgen >";

    #[cfg(feature = "benchmark")]
    println!("message build took: {:.2?}\n\n", now.elapsed());

    // return
    escape_markdown_v2(&msg)
}

async fn get_meals(requested_date: DateTime<Local>) -> DayMeals {
    // returns meals struct either from cache,
    // or starts html request, parses data; returns data and also triggers saving to cache
    #[cfg(feature = "benchmark")]
    let now = Instant::now();

    // url parameters
    let loc = 140;
    let requested_date_urlfmt = build_req_date_string(requested_date);
    let url_params = format!("location={}&date={}", loc, requested_date_urlfmt);

    // try to read from cache
    match File::open(format!("cached_data/{}.json", &url_params)).await {
        // cached file exists, use that
        Ok(mut file) => {
            let mut json_text = String::new();
            file.read_to_string(&mut json_text).await.unwrap();

            let day_meals: DayMeals = serde_json::from_str(&json_text).unwrap();

            #[cfg(feature = "benchmark")]
            println!("json deser took: {:.2?}", now.elapsed());

            day_meals
        }
        // no cached file, use reqwest
        Err(_) => {
            // retrieve HTML
            let html_text = reqwest_get_html_text(&url_params).await;

            #[cfg(feature = "benchmark")]
            println!("req return took: {:.2?}", now.elapsed());

            // extract data to struct
            let day_meals = extract_data_from_html(&html_text, requested_date).await;

            // save struct to cache
            save_data_to_cache(&html_text, &day_meals, &url_params).await;

            // return struct
            day_meals
        }
    }
}

async fn reqwest_get_html_text(url_params: &String) -> String {
    // requests html from server for chosen url params and returns html_text string

    let url_base: String =
        "https://www.studentenwerk-leipzig.de/mensen-cafeterien/speiseplan?".to_owned();

    reqwest::get(url_base + url_params)
        .await
        .expect("URL request failed")
        .text()
        .await
        .unwrap()
}

async fn extract_data_from_html(html_text: &str, requested_date: DateTime<Local>) -> DayMeals {
    #[cfg(feature = "benchmark")]
    let now = Instant::now();

    let mut v_meal_groups: Vec<MealGroup> = Vec::new();

    let document = Html::parse_fragment(html_text);

    // retrieving reported date and comparing to requested date
    let date_sel = Selector::parse(r#"select#edit-date>option[selected='selected']"#).unwrap();
    let received_date_human = document.select(&date_sel).next().unwrap().inner_html();

    let sub = received_date_human.split(' ').nth(1).unwrap();
    let received_date = NaiveDate::parse_from_str(sub, "%d.%m.%Y").unwrap();

    if received_date != requested_date.naive_local().date() {
        return DayMeals {
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
    #[cfg(feature = "benchmark")]
    println!("parsing took: {:.2?}", now.elapsed());

    DayMeals {
        date: received_date_human,
        meal_groups: v_meal_groups,
    }
}

async fn save_data_to_cache(html_text: &String, day_meals: &DayMeals, url_params: &String) {
    // writes html_text and day_meals struct to cache files

    // checks cache dir existence, and creates it if not found
    std::fs::create_dir_all("cached_data/").expect("failed to create data cache dir");

    // saving html content (string comparison is faster than hashing)
    let mut html_file = File::create(format!("cached_data/{}", &url_params))
        .await
        .expect("failed to create a cache file");
    html_file
        .write_all(html_text.as_bytes())
        .await
        .expect("failed to write received data to cache");

    let mut json_file = File::create(format!("cached_data/{}.json", &url_params))
        .await
        .expect("failed to create a json cache file"); //"cached_data/".to_owned() + &url_params).await.expect("failed to create a cache file");
    json_file
        .write_all(serde_json::to_string(&day_meals).unwrap().as_bytes())
        .await
        .expect("failed to write to a json file")
}

fn build_req_date_string(requested_date: DateTime<Local>) -> String {
    let (year, month, day) = (
        requested_date.year(),
        requested_date.month(),
        requested_date.day(),
    );

    let out: String = format!("{:04}-{:02}-{:02}", year, month, day);
    out
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

pub async fn prefetch() -> bool {
    // will be run periodically: requests all possible dates (heute/morgen/ueb) and creates/updates caches
    #[cfg(feature = "benchmark")]
    let now = Instant::now();

    let mut days: Vec<DateTime<Local>> = Vec::new();
    // ugly hardcoded crap. Unfortunately I think this is the most readable.
    // push today/tomorrow/tomorrowier to prefetch days, while dancing around Sat/Sun
    match chrono::Local::now().weekday() {
        Weekday::Thu => {
            // date for 'heute' => thursday
            days.push(chrono::Local::now());
            // date for 'morgen' => friday
            days.push(chrono::Local::now() + Duration::days(1));
            // date for 'uebermorgen' => monday
            days.push(chrono::Local::now() + Duration::days(4));
        }
        Weekday::Fri => {
            // date for 'heute' => friday
            days.push(chrono::Local::now());
            // date for 'morgen'/'uebermorgen' => monday
            days.push(chrono::Local::now() + Duration::days(3));
        }
        Weekday::Sat => {
            // date for 'heute'/'morgen'/'uebermorgen' => monday
            days.push(chrono::Local::now() + Duration::days(2));
        }
        Weekday::Sun => {
            // date for 'heute'/'morgen' => monday
            days.push(chrono::Local::now() + Duration::days(1));
            // date for 'uebermorgen' => tuesday
            days.push(chrono::Local::now() + Duration::days(2));
        }
        _ => {
            // Monday/Tuesday/Wednesday behave as you'd expect.
            days.push(chrono::Local::now());
            days.push(chrono::Local::now() + Duration::days(1));
            days.push(chrono::Local::now() + Duration::days(2));
        }
    }

    #[cfg(feature = "benchmark")]
    println!("date sel took: {:.2?}\n", now.elapsed());

    let loc = 140;

    // add task handles to vec so that they can be awaited after spawing
    let mut handles = Vec::new();

    // spawning task for every day
    for day in days {
        // too lazy to do this properly

        handles.push(tokio::spawn(prefetch_for_day(day, loc)))
    }

    let mut today_changed = false;
    // awaiting results of every task
    for handle in handles {
        if handle.await.unwrap() && !today_changed {
            today_changed = true;
        }
    }

    today_changed
}

async fn prefetch_for_day(day: DateTime<Local>, loc: i32) -> bool {
    #[cfg(feature = "benchmark")]
    println!("starting req for {}", day.weekday());
    #[cfg(feature = "benchmark")]
    let now = Instant::now();

    let requested_date_urlfmt = build_req_date_string(day);
    let url_params = format!("location={}&date={}", loc, requested_date_urlfmt);

    // getting data from server
    let html_text = reqwest_get_html_text(&url_params).await;

    #[cfg(feature = "benchmark")]
    println!("got {} data after {:.2?}", day.weekday(), now.elapsed());

    match File::open("cached_data/".to_owned() + &url_params).await {
        // file exists, check if contents match
        Ok(mut file) => {
            let mut contents = String::new();
            file.read_to_string(&mut contents).await.unwrap();

            // cache is outdated -> overwrite
            if contents != html_text {
                let day_meals = extract_data_from_html(&html_text, day).await;
                save_data_to_cache(&html_text, &day_meals, &url_params).await;

                #[cfg(feature = "benchmark")]
                println!("{}: replaced", day.weekday());

                // cache was updated - return true if "today" was updated
                day.weekday() == chrono::Local::now().weekday()
            } else {
                false
            }
        }
        // cache file doesnt exist, create it
        Err(_) => {
            let day_meals = extract_data_from_html(&html_text, day).await;
            save_data_to_cache(&html_text, &day_meals, &url_params).await;

            // return true if cache for today was updated
            day.weekday() == chrono::Local::now().weekday()
        }
    }
}
