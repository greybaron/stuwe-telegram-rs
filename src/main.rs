use core::panic;
use std::{collections::HashMap, sync::Arc};

use chrono::{Datelike, Duration, Timelike, Weekday};

use log::LevelFilter;
use teloxide::{
    dispatching::dialogue::GetChatId, prelude::*, types::ParseMode,
    utils::command::BotCommands,
};

mod errors;
use errors::TimeParseError;

use rusqlite::{Connection, Result};
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler}; //, JobSchedulerError};

#[macro_use]
extern crate lazy_static;
use regex::Regex;

// pretty lazy
#[derive(Debug, Copy, Clone)]
struct SendMsgJob {
    chatid: i64,
    hour: u32,
    minute: u32,
}

#[tokio::main]
async fn main() {
    let (tx, rx): (
        broadcast::Sender<SendMsgJob>,
        broadcast::Receiver<SendMsgJob>,
    ) = broadcast::channel(10);

    pretty_env_logger::formatted_builder()
        .filter_level(LevelFilter::Info)
        .init();
    log::info!("Starting command bot...");

    let bot = Bot::from_env(); //.parse_mode(ParseMode::MarkdownV2);

    let c_bot = bot.clone();

    let job_uuids: Arc<RwLock<HashMap<i64, uuid::Uuid>>> = Arc::new(RwLock::new(HashMap::new())); //HashMap::new();
    let j_uuids1 = job_uuids.clone();
    let j_uuids2 = job_uuids.clone();
    
    tokio::spawn(async move {
        init_task_scheduler(c_bot, rx, j_uuids1).await;
    });

    Command::repl(bot, move |bot, msg, cmd| {
        let j_uuids2 = j_uuids2.clone();
        answer(bot, msg, cmd, j_uuids2, tx.clone())
    })
    .await;
}

fn save_task_to_db(msg_job: SendMsgJob) -> rusqlite::Result<()> {
    let conn = Connection::open("jobs.db")?;
    conn.execute(
        "replace into jobs (chatid, hour, minute)
    values (?1, ?2, ?3)",
        (msg_job.chatid, msg_job.hour, msg_job.minute),
    )?;

    Ok(())
}

fn load_tasks_db() -> Result<Vec<SendMsgJob>> {
    let mut tasks = Vec::new();
    let conn = Connection::open("jobs.db")?;

    conn.execute(
        "create table if not exists jobs (
            chatid integer not null unique primary key,
            hour integer not null,
            minute integer not null
        )",
        (),
    )?;

    let mut stmt = conn.prepare("SELECT chatid, hour, minute FROM jobs")?;
    let job_iter = stmt.query_map([], |row| {
        Ok(SendMsgJob {
            chatid: row.get(0)?,
            hour: row.get(1)?,
            minute: row.get(2)?,
        })
    })?;

    for job in job_iter {
        tasks.push(job.unwrap())
    }

    Ok(tasks)
}

async fn load_job(bot: Bot, sched: &JobScheduler, task: SendMsgJob) -> uuid::Uuid {
    let de_time = chrono::Local::now()
        .with_hour(task.hour)
        .unwrap()
        .with_minute(task.minute)
        .unwrap()
        .with_second(0)
        .unwrap();
    let utc_time = de_time.naive_utc();

    let job = Job::new_async(
        format!(
            "0 {} {} * * Mon,Tue,Wed,Thu,Fri",
            utc_time.minute(),
            utc_time.hour()
        )
        .as_str(),
        move |_uuid, mut _l| {
            // help
            let bot = bot.clone();
            Box::pin(async move {
                bot.send_message(
                    ChatId(task.chatid),
                    format!("message for {}:{}", task.hour, task.minute),
                )
                .await
                .unwrap();
            })
        },
    )
    .unwrap();

    let uuid = job.guid();
    sched.add(job).await.unwrap();
    uuid
}

// async fn unregister_job(bot: Bot, sched: &JobScheduler, chatid: i64) {

// }

async fn init_task_scheduler(bot: Bot, mut job_rx: broadcast::Receiver<SendMsgJob>, job_uuids: Arc<RwLock<HashMap<i64, uuid::Uuid>>>) {
    let tasks = load_tasks_db();
    let sched = JobScheduler::new().await.unwrap();

    for task in tasks.unwrap() {
        let bot = bot.clone();
        // let
        let chatid = task.chatid;
        let uuid = load_job(bot, &sched, task).await;
        // let chatid = task.chatid;
        // job_uuids.push(uuid.await);

        // let ok = job_uuids.write().await;

        job_uuids.write().await.insert(chatid, uuid);
    }

    sched.start().await.unwrap();

    // set/update send time
    while let Ok(msg_job) = job_rx.recv().await {
        println!("got new job: {:?}", msg_job);
        // old job exists:
        if let Some(old_uuid) = job_uuids.read().await.get(&msg_job.chatid) {
            println!("old job has uuid: {}", old_uuid);

            // unload old job
            sched.context.job_delete_tx.send(*old_uuid).unwrap();
            println!("old job unloaded");

            // kill uuid
            job_uuids.write().await.remove(&msg_job.chatid).unwrap();
            println!("old job uuid removed")
        } else {
            println!("no previous job found");
        }

        // save new data to db
        save_task_to_db(msg_job).unwrap();

        // create or replace db entry
        let new_uuid = load_job(bot.clone(), &sched, msg_job).await;

        println!("new job has uuid: {}", new_uuid);
        // insert new job uuid
        job_uuids.write().await.insert(msg_job.chatid, new_uuid);
    }

    println!("post while let some");
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    #[command(description = "Gerichte für heute")]
    Heute,
    #[command(description = "Gerichte für morgen\n")]
    Morgen,
    #[command(description = "off")]
    Uebermorgen,
    #[command(description = "automat. Nachrichten AN")]
    Subscribe,
    #[command(description = "autom. Nachrichten AUS")]
    Unsubscribe,
    #[command(description = "off")]
    Changetime,
    #[command(description = "off")]
    Start,
}

async fn answer(
    bot: Bot,
    msg: Message,
    cmd: Command,
    job_uuids: Arc<RwLock<HashMap<i64, uuid::Uuid>>>,
    job_tx: broadcast::Sender<SendMsgJob>,
) -> ResponseResult<()> {
    match cmd {
        Command::Start => {
            let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";

            if job_uuids.read().await.get(&msg.chat.id.0).is_none() {
                let first_msg_job = SendMsgJob {
                    chatid: msg.chat.id.0,
                    hour: 6,
                    minute: 0,
                };

                job_tx.send(first_msg_job).unwrap();
            }


            bot.send_message(msg.chat.id, Command::descriptions().to_string() + subtext)
                .await?

        }
        Command::Heute => {
            // let msg =
            bot.send_message(msg.chat.id, "Heute *fett* _und_").await?
        }
        Command::Morgen => bot.send_message(msg.chat.id, "Morgen").await?,
        Command::Uebermorgen => bot.send_message(msg.chat.id, "Morgen").await?,
        Command::Subscribe => bot.send_message(msg.chat.id, "Subscribe").await?,
        Command::Unsubscribe => bot.send_message(msg.chat.id, "Unsubscribe").await?,
        Command::Changetime => {
            let send_result;

            // let (hour, minute) = parse_time(msg.text());
            match parse_time(msg.text().unwrap()) {
                Ok((hour, minute)) => {
                    println!("success");

                    let new_msg_job = SendMsgJob {
                        chatid: msg.chat.id.0,
                        hour,
                        minute,
                    };
        
                    match save_task_to_db(new_msg_job) {
                        Ok(()) => {
                            send_result = bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/changetime [Zeit] zum Ändern\n/unsubscribe zum Deaktivieren", hour, minute)).await?;
                            println!("task saved to db");
                            job_tx.send(new_msg_job).unwrap();
                            println!("SENT JOB MF")
                        }
                        Err(e) => {
                            panic!("failed saving to db: {}", e)
                        }
                    }

                    // job_tx.send(msg_job).unwrap();
                },
                Err(e) => {
                    match e {
                        TimeParseError::NoTimePassed => {
                            send_result = bot.send_message(msg.chat.id, "Bitte Zeit angeben\n/changetime [Zeit]").await?;
                        },
                        TimeParseError::InvalidTimePassed => {
                            send_result = bot.send_message(msg.chat.id, "Eingegebene Zeit ist ungültig.").await?;
                        },
                    }
                }
            }
            send_result

        }
    };

    Ok(())
}

fn parse_time(txt: &str) -> Result<(u32, u32), TimeParseError> {


    if let Some(first_arg) = txt.split(' ').nth(1) {
        lazy_static! {
            static ref RE: Regex = Regex::new("^([01]?[0-9]|2[0-3]):([0-5][0-9])").unwrap();
        }

        if !RE.is_match(first_arg) {
            Err(TimeParseError::InvalidTimePassed)
        } else {
            let caps = RE.captures(first_arg).unwrap();

            let hour = caps.get(1).unwrap().as_str().parse::<u32>().unwrap();
            let minute = caps.get(2).unwrap().as_str().parse::<u32>().unwrap();

            Ok((hour, minute))


        }
    } else {
        Err(TimeParseError::NoTimePassed)
    }
}

// async fn build_chat_message(mode: i64) -> String {
//     #[cfg(feature = "benchmark")]
//     let now = Instant::now();

//     let mut msg: String = String::new();

//     // get requested date
//     let mut requested_date = chrono::Local::now() + Duration::days(mode);
//     let mut date_raised_by_days = 0;

//     match requested_date.weekday() {
//         // sat -> change req_date to mon
//         Weekday::Sat => {
//             requested_date += Duration::days(2);
//             date_raised_by_days = 2;
//         }
//         Weekday::Sun => {
//             requested_date += Duration::days(1);
//             date_raised_by_days = 1;
//         }
//         _ => {
//             // Any other weekday is fine, nothing to do
//         }
//     }

//     #[cfg(feature = "benchmark")]
//     println!("req setup took: {:.2?}", now.elapsed());

//     // retrieve meals
//     let day_meals = get_meals(requested_date).await;

//     // start message formatting
//     #[cfg(feature = "benchmark")]
//     let now = Instant::now();

//     // if mode=0, then "today" was requested. So if date_raised_by_days is != 0 AND mode=0, append warning
//     let future_day_info = if mode == 0 && date_raised_by_days == 1 {
//         "(Morgen)"
//     } else if mode == 0 && date_raised_by_days == 2 {
//         "(Übermorgen)"
//     } else {
//         ""
//     };

//     // insert date+future day info into msg
//     msg += &format!(
//         "{} {}\n",
//         markdown::italic(&day_meals.date),
//         future_day_info
//     );

//     // loop over meal groups
//     for meal_group in day_meals.meal_groups {
//         let mut price_is_shared = true;
//         let price_first_submeal = &meal_group.sub_meals.first().unwrap().price;

//         for sub_meal in &meal_group.sub_meals {
//             if &sub_meal.price != price_first_submeal {
//                 price_is_shared = false;
//                 break;
//             }
//         }

//         // Bold type of meal (-group)
//         msg += &format!("\n{}\n", markdown::bold(&meal_group.meal_type));

//         // loop over meals in meal group
//         for sub_meal in &meal_group.sub_meals {
//             // underlined single or multiple meal name
//             msg += &format!(" • {}\n", markdown::underline(&sub_meal.name));

//             // loop over ingredients of meal
//             for ingredient in &sub_meal.additional_ingredients {
//                 // appending ingredient to msg
//                 msg += &format!("     + {}\n", markdown::italic(ingredient))
//             }
//             // appending price
//             if !price_is_shared {
//                 msg += &format!("   {}\n", sub_meal.price);
//             }
//         }
//         if price_is_shared {
//             msg += &format!("   {}\n", price_first_submeal);
//         }
//     }

//     msg += "\n < /heute >  < /morgen >\n < /uebermorgen >";

//     #[cfg(feature = "benchmark")]
//     println!("message build took: {:.2?}\n\n", now.elapsed());

//     // return
//     escape_markdown_v2(&msg)
// }
