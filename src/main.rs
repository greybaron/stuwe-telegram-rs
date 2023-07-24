mod data_types;
use data_types::{JobHandlerTask, JobType, RegistrationEntry, TimeParseError};
mod stuwe_parser;
use stuwe_parser::{build_meal_message};


use chrono::Timelike;
use log::log_enabled;
use regex::Regex;
use rusqlite::{params, Connection, Result};
use std::{collections::HashMap, env, error::Error, time::Instant};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, Me, ParseMode},
    utils::{command::BotCommands, markdown},
};
use tokio::sync::broadcast;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

#[macro_use]
extern crate lazy_static;

#[tokio::main]
async fn main() {
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module(
            "stuwe_telegram_rs",
            if env::var(pretty_env_logger::env_logger::DEFAULT_FILTER_ENV).unwrap_or_default()
                == "debug"
            {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            },
        )
        .init();

    // pretty_env_logger::
    log::info!("Starting command bot...");

    let mensen: HashMap<&str, u8> = HashMap::from([
        ("Cafeteria Dittrichring", 153),
        ("Mensaria am Botanischen Garten", 127),
        ("Mensa Academica", 118),
        ("Mensa am Park", 106),
        ("Mensa am Elsterbecken", 115),
        ("Mensa am Medizincampus", 162),
        ("Mensa Peterssteinweg", 111),
        ("Mensa Schönauer Straße", 140),
        ("Mensa Tierklinik", 170),
    ]);

    let mensen_c = mensen.clone();

    if !log_enabled!(log::Level::Debug) {
        log::info!("Set env variable 'RUST_LOG=debug' for performance metrics");
    }

    let (registration_tx, job_rx): (
        broadcast::Sender<JobHandlerTask>,
        broadcast::Receiver<JobHandlerTask>,
    ) = broadcast::channel(10);
    let registration_tx2 = registration_tx.clone();

    let (query_registration_tx, _): (broadcast::Sender<Option<RegistrationEntry>>, _) =
        broadcast::channel(10);
    // there is effectively only one tx and rx, however since rx cant be passed to the command_handler,
    // tx has to be cloned, and passed to both (inside command_handler it will be resubscribed to rx)
    let query_registration_tx_tasksched = query_registration_tx.clone();

    let bot = Bot::from_env();
    let bot_tasksched = bot.clone();

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(command_handler))
        .branch(Update::filter_callback_query().endpoint(callback_handler));

    // chat_id -> opt(job_uuid), mensa_id
    // every user has a mensa_id, but only users with auto send have a job_uuid
    let loaded_user_data: HashMap<i64, RegistrationEntry> = HashMap::new();

    tokio::spawn(async move {
        log::info!("Starting task scheduler...");
        init_task_scheduler(
            bot_tasksched,
            registration_tx2,
            job_rx,
            query_registration_tx_tasksched,
            loaded_user_data,
            mensen_c,
        )
        .await;
    });

    // passing a receiver doesnt work for some reason, so sending query_registration_tx and resubscribing to get rx
    let command_handler_deps = dptree::deps![mensen, registration_tx, query_registration_tx]; //, registration_tx, job_query_rx.resubscribe()];
    Dispatcher::builder(bot, handler)
        .dependencies(command_handler_deps)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn callback_handler(
    bot: Bot,
    q: CallbackQuery,
    mensen: HashMap<&str, u8>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(mensa_name) = q.data {
        // acknowledge callback query to remove the loading alert
        bot.answer_callback_query(q.id).await?;

        if let Some(Message { id, chat, .. }) = q.message {
            let new_registration_data =
            // fuck no
            if mensa_name.starts_with('#') {
                let mensa_name = &mensa_name.strip_prefix('#').unwrap();
                // replace mensa selection message with selected mensa
                bot.edit_message_text(chat.id, id, format!("Gewählte Mensa: {}", markdown::bold(mensa_name))).parse_mode(ParseMode::MarkdownV2).await.unwrap();

                JobHandlerTask {
                    job_type: JobType::UpdateRegistration,
                    chat_id: Some(chat.id.0),
                    mensa_id: Some(*mensen.get(mensa_name).unwrap()),
                    hour: None,
                    minute: None,
                }
            }
            else {
                let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";
                // replace mensa selection message with list of commands
                bot.edit_message_text(chat.id, id, Command::descriptions().to_string() + subtext)
                .await?;

                bot.send_message(chat.id, format!("Plan der {} wird ab jetzt automatisch an Wochentagen *06:00 Uhr* gesendet\\.\n\nÄndern mit /uhrzeit [Zeit]", markdown::bold(&mensa_name)))
                .parse_mode(ParseMode::MarkdownV2).await?;

                JobHandlerTask {
                    job_type: JobType::Register,
                    chat_id: Some(chat.id.0),
                    mensa_id: Some(*mensen.get(mensa_name.as_str()).unwrap()),
                    hour: Some(6),
                    minute: Some(0),
                }
            };

            registration_tx.send(new_registration_data).unwrap();
        }
    }

    Ok(())
}

fn make_query_data(chat_id: i64) -> JobHandlerTask {
    JobHandlerTask {
        job_type: JobType::QueryRegistration,
        chat_id: Some(chat_id),
        mensa_id: None,
        hour: None,
        minute: None,
    }
}

async fn command_handler(
    bot: Bot,
    msg: Message,
    me: Me,
    mensen: HashMap<&str, u8>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut query_registration_rx = query_registration_tx.subscribe();
    const NO_DB_MSG: &str = "Bitte zuerst /start ausführen";

    if let Some(text) = msg.text() {
        match BotCommands::parse(text, me.username()) {
            Ok(Command::Heute) => {
                let now = Instant::now();
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();

                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    bot.send_message(msg.chat.id, build_meal_message(0, registration.1).await)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;

                    log::debug!("generating Heute msg took: {:.2?}", now.elapsed());
                } else {
                    // every user has a registration (starting the chat always calls /start)
                    // if this is none, it most likely means the DB was wiped)
                    // forcing reregistration is better than crashing the bot, no data will be overwritten anyways
                    // but copy pasting this everywhere is ugly
                    bot.send_message(msg.chat.id, NO_DB_MSG)
                        .await?;
                }
            }
            Ok(Command::Morgen) => {
                let now = Instant::now();
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();

                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    bot.send_message(msg.chat.id, build_meal_message(1, registration.1).await)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;

                    log::debug!("generating Morgen msg took: {:.2?}", now.elapsed());
                } else {
                    bot.send_message(msg.chat.id, "NO_DB_MSG")
                        .await?;
                }
            }
            Ok(Command::Uebermorgen) => {
                let now = Instant::now();
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();

                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    bot.send_message(msg.chat.id, build_meal_message(1, registration.1).await)
                        .parse_mode(ParseMode::MarkdownV2)
                        .await?;

                    log::debug!("generating Morgen msg took: {:.2?}", now.elapsed());
                } else {
                    bot.send_message(msg.chat.id, "Bitte zuerst /start ausführen")
                        .await?;
                }
            }
            Ok(Command::Start) => {
                let keyboard = make_mensa_keyboard(mensen, false);
                bot.send_message(msg.chat.id, "Mensa auswählen:")
                    .reply_markup(keyboard)
                    .await?;
            }
            Ok(Command::Mensa) => {
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();
                if query_registration_rx.recv().await.unwrap().is_some() {
                    let keyboard = make_mensa_keyboard(mensen, true);
                    bot.send_message(msg.chat.id, "Mensa auswählen:")
                        .reply_markup(keyboard)
                        .await?;
                } else {
                    bot.send_message(msg.chat.id, "Bitte zuerst /start ausführen")
                        .await?;
                }
                
            }
            Ok(Command::Subscribe) => {
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();
                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();
                let uuid = registration.0;//.unwrap().expect("try to operate on non-existing registration");

                if uuid.is_some() {
                    bot.send_message(
                        msg.chat.id,
                        "Automatische Nachrichten sind schon aktiviert.",
                    )
                    .await?;
                }
                else {
                    bot.send_message(
                        msg.chat.id,
                        "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /uhrzeit [Zeit]",
                    )
                    .await?;

                    let registration_job = JobHandlerTask {
                        job_type: JobType::UpdateRegistration,
                        chat_id: Some(msg.chat.id.0),
                        mensa_id: None,
                        hour: Some(6),
                        minute: Some(0),
                    };

                    registration_tx.send(registration_job).unwrap();
                }
                } else {
                    bot.send_message(msg.chat.id, "Bitte zuerst /start ausführen")
                        .await?;
                }
            }

            Ok(Command::Unsubscribe) => {
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();
                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    let uuid = registration.0;

                    if uuid.is_none() {
                        bot.send_message(
                            msg.chat.id,
                            "Automatische Nachrichten waren bereits deaktiviert.",
                        )
                        .await?;

                    } else {
                        bot.send_message(
                            msg.chat.id,
                            "Plan wird nicht mehr automatisch gesendet.",
                        )
                        .await?;

                        registration_tx.send(JobHandlerTask {
                            job_type: JobType::Unregister,
                            chat_id: Some(msg.chat.id.0),
                            mensa_id: None,
                            hour: None,
                            minute: None,
                        }).unwrap();
                    }
                } else {
                    bot.send_message(msg.chat.id, "Bitte zuerst /start ausführen")
                        .await?;
                }
            }
            Ok(Command::Uhrzeit) => {
                registration_tx.send(make_query_data(msg.chat.id.0)).unwrap();
                if query_registration_rx.recv().await.unwrap().is_some() {
                    match parse_time(msg.text().unwrap()) {
                        Ok((hour, minute)) => {
                            bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/unsubscribe zum Deaktivieren", hour, minute)).await?;
    
                            let registration_job = JobHandlerTask {
                                job_type: JobType::UpdateRegistration,
                                chat_id: Some(msg.chat.id.0),
                                mensa_id: None,
                                hour: Some(hour),
                                minute: Some(minute),
                            };
                            registration_tx.send(registration_job).unwrap();
                        }
                        Err(TimeParseError::NoTimePassed) => {
                            bot.send_message(msg.chat.id, "Bitte Zeit angeben\n/uhrzeit [Zeit]")
                                .await?;
                        }
                        Err(TimeParseError::InvalidTimePassed) => {
                            bot.send_message(msg.chat.id, "Eingegebene Zeit ist ungültig.")
                                .await?;
                        }
                    };
                } else {
                    bot.send_message(msg.chat.id, "Bitte zuerst /start ausführen")
                        .await?;
                }
            }
            Err(_) => {
                bot.send_message(msg.chat.id, "Ungültiger Befehl").await?;
            }
        }
    }

    Ok(())
}

fn make_mensa_keyboard(mensen: HashMap<&str, u8>, only_mensa_upd: bool) -> InlineKeyboardMarkup {
    let mut keyboard = Vec::new();

    // shitty
    for mensa in mensen {
        if only_mensa_upd {
            keyboard.push([InlineKeyboardButton::callback(
                mensa.0,
                format!("#{}", mensa.0),
            )]);
        } else {
            keyboard.push([InlineKeyboardButton::callback(mensa.0, mensa.0)]);
        }
    }
    InlineKeyboardMarkup::new(keyboard)
}

fn init_db_record(msg_job: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open("../../jobs.db")?;
    let mut stmt = conn
        .prepare_cached(
            "replace into registrations (chat_id, mensa_id, hour, minute)
            values (?1, ?2, ?3, ?4)",
        )
        .unwrap();

    stmt.execute(params![
        msg_job.chat_id,
        msg_job.mensa_id,
        msg_job.hour.unwrap(),
        msg_job.minute.unwrap()
    ])?;
    Ok(())
}

// fn update_db_row(chat_id: i64, mensa_id: Option<u8>, hour: Option<u32>, minute: Option<u32>) -> rusqlite::Result<()> {
fn update_db_row(data: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open("../../jobs.db")?;

    // could be better but eh
    let mut update_mensa_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET mensa_id = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    let mut upd_hour_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET hour = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    let mut upd_min_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET minute = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    if let Some(mensa_id) = data.mensa_id {
        update_mensa_stmt.execute(params![data.chat_id, mensa_id])?;
    };

    if let Some(hour) = data.hour {
        upd_hour_stmt.execute(params![data.chat_id, hour])?;
    };

    if let Some(minute) = data.minute {
        upd_min_stmt.execute(params![data.chat_id, minute])?;
    };

    Ok(())
}

fn task_db_kill_auto(chat_id: i64) -> rusqlite::Result<()> {
    let conn = Connection::open("../../jobs.db")?;
    let mut stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET hour = NULL, minute = NULL
            WHERE chat_id = ?1",
        )
        .unwrap();

    stmt.execute(params![chat_id])?;

    Ok(())
}

fn get_all_tasks_db() -> Vec<JobHandlerTask> {
    let mut tasks: Vec<JobHandlerTask> = Vec::new();
    let conn = Connection::open("../../jobs.db").unwrap();

    // ensure db table exists
    conn.prepare(
        "create table if not exists registrations (
        chat_id integer not null unique primary key,
        mensa_id integer not null,
        hour integer,
        minute integer
    )",
    )
    .unwrap()
    .execute([])
    .unwrap();

    let mut stmt = conn
        .prepare_cached("SELECT chat_id, mensa_id, hour, minute FROM registrations")
        .unwrap();

    let job_iter = stmt
        .query_map([], |row| {
            Ok(JobHandlerTask {
                job_type: JobType::UpdateRegistration,
                chat_id: row.get(0)?,
                mensa_id: row.get(1)?,
                hour: row.get(2)?,
                minute: row.get(3)?,
            })
        })
        .unwrap();

    for job in job_iter {
        tasks.push(job.unwrap())
    }

    tasks
}

async fn load_job(bot: Bot, sched: &JobScheduler, task: JobHandlerTask) -> Option<Uuid> {
    // return if no time is set
    task.hour?;

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
                bot.send_message(
                    ChatId(task.chat_id.unwrap()),
                    build_meal_message(0, task.mensa_id.unwrap()).await,
                )
                .parse_mode(ParseMode::MarkdownV2)
                .await
                .unwrap();
            })
        },
    )
    .unwrap();

    let uuid = job.guid();
    sched.add(job).await.unwrap();

    Some(uuid)
}

async fn init_task_scheduler(
    bot: Bot,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    mut job_rx: broadcast::Receiver<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    mut loaded_user_data: HashMap<i64, RegistrationEntry>,
    mensen: HashMap<&str, u8>,
) {
    let tasks_from_db = get_all_tasks_db();
    let sched = JobScheduler::new().await.unwrap();

    let mensen_ids: Vec<u8> = mensen.values().copied().collect();

    // // run cache update every 5 minutes
    // let cache_and_broadcast_job = Job::new_async("0 1/5 * * * *", move |_uuid, mut _l| {
    //     log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache");

    //     let registration_tx = registration_tx.clone();
    //     let mensen_ids = mensen_ids.clone();

    //     Box::pin(async move {
    //         let today_changed_mensen = update_cache(&mensen_ids).await;

    //         for mensa in today_changed_mensen {
    //             registration_tx
    //                 .send(JobHandlerTask {
    //                     job_type: JobType::BroadcastUpdate,
    //                     chat_id: None,
    //                     mensa_id: Some(mensa),
    //                     hour: None,
    //                     minute: None,
    //                 })
    //                 .unwrap();
    //         }
    //     })
    // })
    // .unwrap();
    // sched.add(cache_and_broadcast_job).await.unwrap();

    for task in tasks_from_db {
        let bot = bot.clone();

        let uuid = load_job(bot, &sched, task.clone()).await;
        loaded_user_data.insert(task.chat_id.unwrap(), (uuid, task.mensa_id.unwrap(), task.hour, task.minute));
    }

    // start scheduler (non blocking)
    sched.start().await.unwrap();

    log::info!(target: "stuwe_telegram_rs::TaskSched", "Ready.");

    // receive job update msg (register/unregister/check existence)
    while let Ok(msg_job) = job_rx.recv().await {
        match msg_job.job_type {
            JobType::Register => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Register: {} for Mensa {}", &msg_job.chat_id.unwrap(), &msg_job.mensa_id.unwrap());
                // creates a new row, or replaces every col with new values
                init_db_record(&msg_job).unwrap();

                if let Some(previous_registration) = loaded_user_data.get(&msg_job.chat_id.unwrap())
                {
                    if let Some(uuid) = previous_registration.0 {
                        sched.context.job_delete_tx.send(uuid).unwrap();
                    }
                };

                // get uuid (here guaranteed to be Some() since default is registration with job)
                let new_uuid = load_job(bot.clone(), &sched, msg_job.clone()).await;

                // insert new job uuid
                loaded_user_data.insert(
                    msg_job.chat_id.unwrap(),
                    (new_uuid, msg_job.mensa_id.unwrap(), msg_job.hour, msg_job.minute),
                );
            }
            JobType::UpdateRegistration => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{}: Changed: {} {}",
                    msg_job.chat_id.unwrap(),
                    if msg_job.mensa_id.is_some() {"Mensa-ID"} else {""},
                    if msg_job.hour.is_some() {"Time"} else {""},
                );
                
                if let Some(previous_registration) = loaded_user_data.get(&msg_job.chat_id.unwrap()) {
                    let new_uuid = if msg_job.hour.is_some() || msg_job.minute.is_some() {
                        if let Some(uuid) = previous_registration.0 {
                            // unload old job if exists
                            sched.context.job_delete_tx.send(uuid).unwrap();
                        }
                        // load new job, return uuid
                        load_job(bot.clone(), &sched, msg_job.clone()).await
                    } else {
                        // no new time was set -> return old job uuid
                        previous_registration.0
                    };
    
                    let mensa_id = if msg_job.mensa_id.is_some() {
                        msg_job.mensa_id.unwrap()
                    } else {
                        previous_registration.1
                    };
    
                    loaded_user_data.insert(msg_job.chat_id.unwrap(), (new_uuid, mensa_id, msg_job.hour, msg_job.minute));
    
                    // update any values that are to be changed, aka are Some()
                    update_db_row(&msg_job).unwrap();
                } else {
                    log::error!("Tried to update non-existent job");
                }
            }

            JobType::Unregister => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Unregister: {}", &msg_job.chat_id.unwrap());

                // unregister is only invoked if existence of job is guaranteed
                let previous_registration =
                    loaded_user_data.get(&msg_job.chat_id.unwrap()).unwrap();

                // unload old job
                sched
                    .context
                    .job_delete_tx
                    .send(previous_registration.0.unwrap())
                    .unwrap();

                // kill uuid from this thing
                loaded_user_data.insert(msg_job.chat_id.unwrap(), (None, previous_registration.1, None, None));

                // delete from db
                task_db_kill_auto(msg_job.chat_id.unwrap()).unwrap();
            }

            JobType::QueryRegistration => {
                // check if uuid exists
                let uuid_opt = loaded_user_data.get(&msg_job.chat_id.unwrap());
                if uuid_opt.is_none() {
                    log::error!(target: "stuwe_telegram_rs::TS::Jobs", "chat_id {} sent a command, but is not in DB", &msg_job.chat_id.unwrap());
                }

                query_registration_tx.send(uuid_opt.copied()).unwrap();
            }
            JobType::BroadcastUpdate => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "TodayMeals changed @Mensa {}", &msg_job.mensa_id.unwrap());
                for registration in &loaded_user_data {

                    let chat_id = *registration.0;
                    let registration_data = registration.1;
                    let mensa_id = registration_data.1;

                    let now = chrono::Local::now();

                    if let (Some(job_hour), Some(job_minute)) = (registration_data.2, registration_data.3) {
                        if mensa_id == msg_job.mensa_id.unwrap() && (job_hour < now.hour() || job_hour == now.hour() && job_minute <= now.minute()) {
                            log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Sent update to {}", chat_id);
    
                            bot.send_message(
                                ChatId(chat_id),
                                format!(
                                    "{}\n{}",
                                    &markdown::bold(&markdown::underline("Planänderung:")),
                                    build_meal_message(0, mensa_id).await
                                ),
                            )
                            .parse_mode(ParseMode::MarkdownV2)
                            .await
                            .unwrap();
                        }
                    }
                }
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
    Uhrzeit,
    #[command(description = "off")]
    Mensa,
    #[command(description = "off")]
    Start,
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
