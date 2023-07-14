mod data_types;
mod stuwe_parser;
use stuwe_parser::{build_meal_message, prefetch};

use data_types::{JobHandlerTask, JobType, TimeParseError};

use chrono::Timelike;
use core::panic;
use log::LevelFilter;
use regex::Regex;
use rusqlite::{params, Connection, Result};
use std::collections::HashMap;
use teloxide::{
    prelude::*,
    types::ParseMode,
    utils::{command::BotCommands, markdown},
};
use tokio::sync::broadcast;
use tokio_cron_scheduler::{Job, JobScheduler};

#[macro_use]
extern crate lazy_static;

#[tokio::main]
async fn main() {
    let (job_tx, job_rx): (
        broadcast::Sender<JobHandlerTask>,
        broadcast::Receiver<JobHandlerTask>,
    ) = broadcast::channel(10);
    let job_tx2 = job_tx.clone();

    let (job_exists_tx, job_exists_rx): (broadcast::Sender<bool>, broadcast::Receiver<bool>) =
        broadcast::channel(10);

    pretty_env_logger::formatted_builder()
        .filter_level(LevelFilter::Info)
        .init();
    log::info!("Starting command bot...");

    let bot = Bot::from_env();
    let c_bot = bot.clone();

    let job_uuids: HashMap<i64, uuid::Uuid> = HashMap::new();

    tokio::spawn(async move {
        init_task_scheduler(c_bot, job_tx, job_rx, job_exists_tx, job_uuids).await;
    });

    Command::repl(bot, move |bot, msg, cmd| {
        answer(bot, msg, cmd, job_tx2.clone(), job_exists_rx.resubscribe())
    })
    .await;
}

fn save_task_to_db(msg_job: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open("../../jobs.db")?;

    let mut stmt = conn
        .prepare_cached(
            "replace into jobs (chatid, hour, minute)
    values (?1, ?2, ?3)",
        )
        .unwrap();

    stmt.execute(params![msg_job.chatid, msg_job.hour, msg_job.minute])?;

    Ok(())
}

fn delete_task_from_db(chatid: i64) -> rusqlite::Result<()> {
    let conn = Connection::open("../../jobs.db")?;
    let mut stmt = conn
        .prepare_cached("delete from jobs where chatid = ?1")
        .unwrap();

    stmt.execute(params![chatid])?;

    Ok(())
}

fn load_tasks_db() -> Result<Vec<JobHandlerTask>> {
    let mut tasks = Vec::new();
    let conn = Connection::open("../../jobs.db")?;
    let mut stmt = conn
        .prepare_cached(
            "create table if not exists jobs (
        chatid integer not null unique primary key,
        hour integer not null,
        minute integer not null
    )",
        )
        .unwrap();

    stmt.execute(())?;

    let mut stmt = conn.prepare_cached("SELECT chatid, hour, minute FROM jobs")?;

    let job_iter = stmt.query_map([], |row| {
        Ok(JobHandlerTask {
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

async fn load_job(bot: Bot, sched: &JobScheduler, task: JobHandlerTask) -> uuid::Uuid {
    // convert de time to utc
    let de_time = chrono::Local::now()
        .with_hour(task.hour.unwrap())
        .unwrap()
        .with_minute(task.minute.unwrap())
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
            let bot = bot.clone();
            Box::pin(async move {
                bot.send_message(ChatId(task.chatid.unwrap()), build_meal_message(0).await)
                    .parse_mode(ParseMode::MarkdownV2)
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

async fn init_task_scheduler(
    bot: Bot,
    job_tx: broadcast::Sender<JobHandlerTask>,
    mut job_rx: broadcast::Receiver<JobHandlerTask>,
    job_exists_tx: broadcast::Sender<bool>,
    mut job_uuids: HashMap<i64, uuid::Uuid>,
) {
    let tasks = load_tasks_db();
    let sched = JobScheduler::new().await.unwrap();

    // always update cache on startup
    prefetch().await;

    // run cache update every 5 minutes
    let cache_job = Job::new_async("1/10 * * * * *", move |_uuid, mut _l| {
        // 0 1/5 * * * *
        let job_tx = job_tx.clone();

        Box::pin(async move {
            println!("prefetch");
            let today_changed = prefetch().await;
            if today_changed {
                println!("bot: today changed");
                job_tx
                    .send(JobHandlerTask {
                        job_type: JobType::BroadcastUpdate,
                        chatid: None,
                        hour: None,
                        minute: None,
                    })
                    .unwrap();
            }
        })
    })
    .unwrap();
    sched.add(cache_job).await.unwrap();

    for task in tasks.unwrap() {
        let bot = bot.clone();

        let j_uuid: uuid::Uuid = load_job(bot, &sched, task.clone()).await;
        job_uuids.insert(task.chatid.unwrap(), j_uuid);
    }

    // start scheduler (non blocking)
    sched.start().await.unwrap();

    // receive job update msg (register/unregister/check existence)
    while let Ok(msg_job) = job_rx.recv().await {
        match msg_job.job_type {
            JobType::Register => {
                // old job exists:
                if let Some(old_uuid) = job_uuids.get(&msg_job.chatid.unwrap()) {
                    println!("Changing job time for chatid: {}", &msg_job.chatid.unwrap());
                    // unload old job
                    sched.context.job_delete_tx.send(*old_uuid).unwrap();

                    // kill uuid
                    job_uuids.remove(&msg_job.chatid.unwrap()).unwrap();
                } else {
                    println!("Registering job for chatid: {}", &msg_job.chatid.unwrap());
                }

                // save new data to db
                save_task_to_db(&msg_job).unwrap();

                // create or replace db entry
                let new_uuid = load_job(bot.clone(), &sched, msg_job.clone()).await;

                // insert new job uuid
                job_uuids.insert(msg_job.chatid.unwrap(), new_uuid);
            }
            JobType::Unregister => {
                println!("Unregistering job for chatid: {}", &msg_job.chatid.unwrap());

                // unregister is only invoked if existence of job is guaranteed
                let old_uuid = job_uuids.get(&msg_job.chatid.unwrap()).unwrap();

                // unload old job
                sched.context.job_delete_tx.send(*old_uuid).unwrap();

                // kill uuid
                job_uuids.remove(&msg_job.chatid.unwrap()).unwrap();

                // delete from db
                delete_task_from_db(msg_job.chatid.unwrap()).unwrap();
            }
            JobType::CheckJobExists => {
                job_exists_tx
                    .send(job_uuids.get(&msg_job.chatid.unwrap()).is_some())
                    .unwrap();
            }
            JobType::BroadcastUpdate => {
                println!("send update pls");
                for registration in &job_uuids {
                    bot.send_message(
                        ChatId(*registration.0),
                        format!(
                            "{}\n{}",
                            &markdown::bold(&markdown::underline("Planänderung:")),
                            build_meal_message(0).await
                        ),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await
                    .unwrap();
                }
                // bot.send
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
    job_tx: broadcast::Sender<JobHandlerTask>,
    mut job_exists_rx: broadcast::Receiver<bool>,
) -> ResponseResult<()> {
    match cmd {
        Command::Start => {
            let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";

            let send_res = bot
                .send_message(msg.chat.id, Command::descriptions().to_string() + subtext)
                .await?;

            let check_exists_task = JobHandlerTask {
                job_type: JobType::CheckJobExists,
                chatid: Some(msg.chat.id.0),
                hour: None,
                minute: None,
            };

            job_tx.send(check_exists_task).unwrap();

            if !job_exists_rx.recv().await.unwrap() {
                let first_msg_job = JobHandlerTask {
                    job_type: JobType::Register,
                    chatid: Some(msg.chat.id.0),
                    hour: Some(6),
                    minute: Some(0),
                };

                job_tx.send(first_msg_job).unwrap();
                bot.send_message(
                    msg.chat.id,
                    "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /changetime [Zeit]",
                )
                .await?;
            } else {
                bot.send_message(msg.chat.id, "Automatische Nachrichten sind aktiviert.")
                    .await?;
            }

            send_res
        }
        Command::Heute => {
            let text = build_meal_message(0).await;
            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::MarkdownV2)
                .await?
        }
        Command::Morgen => {
            let text = build_meal_message(1).await;
            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::MarkdownV2)
                .await?
        }
        Command::Uebermorgen => {
            let text = build_meal_message(2).await;
            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::MarkdownV2)
                .await?
        }
        Command::Subscribe => {
            let send_result;
            let check_exists_task = JobHandlerTask {
                job_type: JobType::CheckJobExists,
                chatid: Some(msg.chat.id.0),
                hour: None,
                minute: None,
            };

            job_tx.send(check_exists_task).unwrap();

            if job_exists_rx.recv().await.unwrap() {
                send_result = bot
                    .send_message(
                        msg.chat.id,
                        "Automatische Nachrichten sind schon aktiviert.",
                    )
                    .await?;
            } else {
                let new_msg_job = JobHandlerTask {
                    job_type: JobType::Register,
                    chatid: Some(msg.chat.id.0),
                    hour: Some(6),
                    minute: Some(0),
                };
                send_result = bot
                    .send_message(
                        msg.chat.id,
                        "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /changetime [Zeit]",
                    )
                    .await?;
                job_tx.send(new_msg_job).unwrap();
            }

            send_result
        }

        Command::Unsubscribe => {
            let check_exists_task = JobHandlerTask {
                job_type: JobType::CheckJobExists,
                chatid: Some(msg.chat.id.0),
                hour: None,
                minute: None,
            };

            job_tx.send(check_exists_task).unwrap();

            if !job_exists_rx.recv().await.unwrap() {
                bot.send_message(
                    msg.chat.id,
                    "Automatische Nachrichten waren bereits deaktiviert.",
                )
                .await?
            } else {
                let kill_msg = JobHandlerTask {
                    job_type: JobType::Unregister,
                    chatid: Some(msg.chat.id.0),
                    hour: None,
                    minute: None,
                };

                job_tx.send(kill_msg).unwrap();
                bot.send_message(msg.chat.id, "Plan wird nicht mehr automatisch gesendet.")
                    .await?
            }
        }

        Command::Changetime => {
            let send_result;

            match parse_time(msg.text().unwrap()) {
                Ok((hour, minute)) => {
                    let new_msg_job = JobHandlerTask {
                        job_type: JobType::Register,
                        chatid: Some(msg.chat.id.0),
                        hour: Some(hour),
                        minute: Some(minute),
                    };

                    match save_task_to_db(&new_msg_job) {
                        Ok(()) => {
                            send_result = bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/unsubscribe zum Deaktivieren", hour, minute)).await?;
                            job_tx.send(new_msg_job).unwrap();
                        }
                        Err(e) => {
                            panic!("failed saving to db: {}", e)
                        }
                    }
                }
                Err(e) => match e {
                    TimeParseError::NoTimePassed => {
                        send_result = bot
                            .send_message(msg.chat.id, "Bitte Zeit angeben\n/changetime [Zeit]")
                            .await?;
                    }
                    TimeParseError::InvalidTimePassed => {
                        send_result = bot
                            .send_message(msg.chat.id, "Eingegebene Zeit ist ungültig.")
                            .await?;
                    }
                },
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
