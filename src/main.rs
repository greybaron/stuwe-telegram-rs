use core::panic;
use std::collections::HashMap;

use chrono::Timelike;

use log::LevelFilter;

use teloxide::{
    prelude::*,
    utils::command::BotCommands,
    types::ParseMode
};



mod data_types;
use data_types::TimeParseError;
mod scraper;

use rusqlite::{Connection, Result};
use tokio::sync::broadcast;
use tokio_cron_scheduler::{Job, JobScheduler}; //, JobSchedulerError};

#[macro_use]
extern crate lazy_static;
use regex::Regex;

#[derive(Debug, Copy, Clone)]
enum JobType {
    Register,
    Unregister,
}

// pretty lazy
#[derive(Debug, Copy, Clone)]
struct SendMsgJob {
    job_type: JobType,
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

    let job_uuids: HashMap<i64, uuid::Uuid> = HashMap::new(); //HashMap::new();
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
            job_type: JobType::Register,
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

async fn init_task_scheduler(bot: Bot, mut job_rx: broadcast::Receiver<SendMsgJob>, mut job_uuids: HashMap<i64, uuid::Uuid>) {
    let tasks = load_tasks_db();
    let sched = JobScheduler::new().await.unwrap();

    Job::new_async(
        "0 1/5 * * * *",

        move |_uuid, mut _l| {
            Box::pin(async move {
                scraper::prefetch().await;
            })
        },
    )
    .unwrap();

    for task in tasks.unwrap() {
        let bot = bot.clone();
        // let
        let chatid = task.chatid;
        let uuid = load_job(bot, &sched, task).await;
        // let chatid = task.chatid;
        // job_uuids.push(uuid.await);

        // let ok = job_uuids.write().await;


        job_uuids.insert(chatid, uuid);
    }

    sched.start().await.unwrap();

    // set/update send time
    while let Ok(msg_job) = job_rx.recv().await {
        match msg_job.job_type {
            JobType::Register => {
                println!("got new job: {:?}", msg_job);
                // old job exists:
                if let Some(old_uuid) = job_uuids.get(&msg_job.chatid) {
                    println!("old job has uuid: {}", old_uuid);

                    // unload old job
                    sched.context.job_delete_tx.send(*old_uuid).unwrap();
                    println!("old job unloaded");

                    // kill uuid
                    job_uuids.remove(&msg_job.chatid).unwrap();
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
                job_uuids.insert(msg_job.chatid, new_uuid);
            },
            JobType::Unregister => {
                // unregister is only passed if existence of job is guaranteed
                let old_uuid = job_uuids.get(&msg_job.chatid).unwrap();

                // unload old job
                sched.context.job_delete_tx.send(*old_uuid).unwrap();
                println!("old job unloaded");

                // kill uuid
                job_uuids.remove(&msg_job.chatid).unwrap();
                println!("old job uuid removed")
            }
        }
    }
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
    job_uuids: HashMap<i64, uuid::Uuid>,
    job_tx: broadcast::Sender<SendMsgJob>,
) -> ResponseResult<()> {
    match cmd {
        Command::Start => {
            let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";

            let send_res = bot.send_message(msg.chat.id, Command::descriptions().to_string() + subtext)
                .await?;

            if job_uuids.get(&msg.chat.id.0).is_none() {
                let first_msg_job = SendMsgJob {
                    job_type: JobType::Register,
                    chatid: msg.chat.id.0,
                    hour: 6,
                    minute: 0,
                };

                job_tx.send(first_msg_job).unwrap();
                bot.send_message(msg.chat.id, "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.").await?;
            } else {
                bot.send_message(msg.chat.id, "Automatische Nachrichten sind aktiviert.").await?;
            }

            send_res
        }
        Command::Heute => {
            let text = scraper::build_chat_message(0).await;
            bot.send_message(msg.chat.id, text).parse_mode(ParseMode::MarkdownV2).await?
        }
        Command::Morgen => {
            let text = scraper::build_chat_message(1).await;
            bot.send_message(msg.chat.id, text).parse_mode(ParseMode::MarkdownV2).await?
        },
        Command::Uebermorgen => {
            let text = scraper::build_chat_message(2).await;
            bot.send_message(msg.chat.id, text).parse_mode(ParseMode::MarkdownV2).await?
        },
        Command::Subscribe => {
            let send_result;

            if job_uuids.get(&msg.chat.id.0).is_some() {
                send_result = bot.send_message(msg.chat.id, "Automatische Nachrichten sind schon aktiviert.").await?;

            } else {
                let new_msg_job = SendMsgJob {
                    job_type: JobType::Register,
                    chatid: msg.chat.id.0,
                    hour: 6,
                    minute: 0,
                };
                send_result = bot.send_message(msg.chat.id, "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.").await?; 
                job_tx.send(new_msg_job).unwrap();
            }


            send_result
            //HIER
        },
        Command::Unsubscribe => {
            let send_result;

            if job_uuids.get(&msg.chat.id.0).is_none() {
                send_result = bot.send_message(msg.chat.id, "Automatische Nachrichten waren bereits deaktiviert.").await?;
            } else {
                let kill_msg = SendMsgJob {
                    job_type: JobType::Unregister,
                    chatid: msg.chat.id.0,
                    hour: 0,
                    minute: 0,
                };

                job_tx.send(kill_msg).unwrap();
                send_result = bot.send_message(msg.chat.id, "Plan wird nicht mehr automatisch gesendet.").await?;
            }

            send_result
        },

        Command::Changetime => {
            let send_result;

            // let (hour, minute) = parse_time(msg.text());
            match parse_time(msg.text().unwrap()) {
                Ok((hour, minute)) => {
                    println!("success");

                    let new_msg_job = SendMsgJob {
                        job_type: JobType::Register,
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