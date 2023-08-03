mod data_backend;
mod data_types;

use data_types::{JobHandlerTask, JobType, RegistrationEntry, TimeParseError};
cfg_if! {
    if #[cfg(feature = "mensimates")] {
        use data_backend::mm_parser::{build_meal_message, get_jwt_token};
    } else {
        use data_backend::stuwe_parser::{build_meal_message, update_cache};
    }
}

use chrono::Timelike;
use log::log_enabled;
use regex_lite::Regex;
use rusqlite::{params, Connection, Result};
use static_init::dynamic;
use std::{
    collections::{BTreeMap, HashMap},
    env,
    error::Error,
    sync::Arc,
    time::Instant,
};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, Me, MessageId, ParseMode},
    utils::{command::BotCommands, markdown},
};
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

#[macro_use]
extern crate cfg_if;

#[tokio::main]
async fn main() {
    let jwt_lock: Arc<RwLock<String>> = Arc::new(RwLock::new(String::from("test")));

    pretty_env_logger::formatted_timed_builder()
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

    let mensen: BTreeMap<&str, u8> = BTreeMap::from([
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

    #[cfg(not(feature = "mensimates"))]
    let mensen_ts = mensen.clone();

    if !log_enabled!(log::Level::Debug) {
        log::info!("Set env variable 'RUST_LOG=debug' for performance metrics");
    }

    let (registration_tx, job_rx): (
        broadcast::Sender<JobHandlerTask>,
        broadcast::Receiver<JobHandlerTask>,
    ) = broadcast::channel(10);
    let registration_tx_ts = registration_tx.clone();

    let (query_registration_tx, _): (broadcast::Sender<Option<RegistrationEntry>>, _) =
        broadcast::channel(10);
    // there is effectively only one tx and rx, however since rx cant be passed to the command_handler,
    // tx has to be cloned, and passed to both (inside command_handler it will be resubscribed to rx)
    let query_registration_tx_ts = query_registration_tx.clone();

    let bot = Bot::from_env();
    let bot_tasksched = bot.clone();

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(command_handler))
        .branch(Update::filter_callback_query().endpoint(callback_handler));

    // chat_id -> opt(job_uuid), mensa_id
    // every user has a mensa_id, but only users with auto send have a job_uuid
    let loaded_user_data: HashMap<i64, RegistrationEntry> = HashMap::new();
    {
        #[cfg(feature = "mensimates")]
        let jwt_lock = jwt_lock.clone();
        tokio::spawn(async move {
            log::info!("Starting task scheduler...");
            init_task_scheduler(
                bot_tasksched,
                registration_tx_ts,
                job_rx,
                query_registration_tx_ts,
                loaded_user_data,
                #[cfg(feature = "mensimates")]
                jwt_lock,
                #[cfg(not(feature = "mensimates"))]
                mensen_ts,
            )
            .await;
        });
    }

    // passing a receiver doesnt work for some reason, so sending query_registration_tx and resubscribing to get rx
    let command_handler_deps =
        dptree::deps![mensen, registration_tx, query_registration_tx, jwt_lock];
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
    mensen: BTreeMap<&str, u8>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    #[cfg(feature = "mensimates")] jwt_lock: Arc<RwLock<String>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut query_registration_rx = query_registration_tx.subscribe();

    if let Some(q_data) = q.data {
        // acknowledge callback query to remove the loading alert
        bot.answer_callback_query(q.id).await?;

        if let Some(Message { id, chat, .. }) = q.message {
            let (cmd, arg) = q_data.split_once(':').unwrap();
            match cmd {
                "m_upd" => {
                    // replace mensa selection message with selected mensa
                    bot.edit_message_text(
                        chat.id,
                        id,
                        format!("Gewählte Mensa: {}", markdown::bold(arg)),
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .await
                    .unwrap();

                    let text = build_meal_message(
                        0,
                        *mensen.get(arg).unwrap(),
                        #[cfg(feature = "mensimates")]
                        jwt_lock,
                    )
                    .await;

                    let keyboard = make_days_keyboard();
                    bot.send_message(chat.id, text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?;

                    let task = JobHandlerTask {
                        job_type: JobType::UpdateRegistration,
                        chat_id: Some(chat.id.0),
                        mensa_id: Some(*mensen.get(arg).unwrap()),
                        hour: None,
                        minute: None,
                        callback_id: None,
                    };
                    registration_tx.send(task).unwrap();
                }
                "m_regist" => {
                    let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";
                    // replace mensa selection message with list of commands
                    bot.edit_message_text(
                        chat.id,
                        id,
                        Command::descriptions().to_string() + subtext,
                    )
                    .await?;

                    bot.send_message(chat.id, format!("Plan der {} wird ab jetzt automatisch an Wochentagen *06:00 Uhr* gesendet\\.\n\nÄndern mit\n/mensa, bzw\\.\n/uhrzeit \\[Zeit\\]", markdown::bold(arg)))
                    .parse_mode(ParseMode::MarkdownV2).await?;

                    let text = build_meal_message(
                        0,
                        *mensen.get(arg).unwrap(),
                        #[cfg(feature = "mensimates")]
                        jwt_lock,
                    )
                    .await;

                    let keyboard = make_days_keyboard();
                    bot.send_message(chat.id, text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?;

                    let task = JobHandlerTask {
                        job_type: JobType::Register,
                        chat_id: Some(chat.id.0),
                        mensa_id: Some(*mensen.get(arg).unwrap()),
                        hour: Some(6),
                        minute: Some(0),
                        callback_id: None,
                    };
                    registration_tx.send(task).unwrap();
                }
                "day" => {
                    registration_tx.send(make_query_data(chat.id.0)).unwrap();
                    let prev_reg_opt = query_registration_rx.recv().await.unwrap();
                    if let Some(prev_registration) = prev_reg_opt {
                        if let Some(callback_id) = prev_registration.4 {
                            // also try to edit callback
                            _ = bot
                                .edit_message_reply_markup(chat.id, MessageId(callback_id))
                                .await;
                        }

                        // also try to remove answered query anyway, as a fallback (the more the merrier)
                        _ = bot.edit_message_reply_markup(chat.id, id).await;

                        // start building message
                        let now = Instant::now();

                        let days_forward = arg.parse::<i64>().unwrap();
                        let day_str = ["Heute", "Morgen", "Übermorgen"]
                            [usize::try_from(days_forward).unwrap()];

                        let text = build_meal_message(
                            days_forward,
                            prev_registration.1,
                            #[cfg(feature = "mensimates")]
                            jwt_lock,
                        )
                        .await;
                        log::debug!("Build {} msg: {:.2?}", day_str, now.elapsed());
                        let now = Instant::now();

                        let keyboard = make_days_keyboard();
                        let msg = bot
                            .send_message(chat.id, text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?;
                        log::debug!("Send {} msg: {:.2?}", day_str, now.elapsed());

                        let task = JobHandlerTask {
                            job_type: JobType::InsertCallbackMessageId,
                            chat_id: Some(chat.id.0),
                            mensa_id: None,
                            hour: None,
                            minute: None,
                            callback_id: Some(msg.id.0),
                        };
                        update_db_row(&task).unwrap();
                        registration_tx.send(task).unwrap();
                    } else {
                        bot.send_message(chat.id, "Bitte zuerst /start ausführen")
                            .await?;
                    }
                }
                _ => panic!("Unknown callback query command: {}", cmd),
            }
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
        callback_id: None,
    }
}

async fn command_handler(
    bot: Bot,
    msg: Message,
    me: Me,
    mensen: BTreeMap<&str, u8>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    #[cfg(feature = "mensimates")] jwt_lock: Arc<RwLock<String>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut query_registration_rx = query_registration_tx.subscribe();
    const NO_DB_MSG: &str = "Bitte zuerst /start ausführen";

    if let Some(text) = msg.text() {
        match BotCommands::parse(text, me.username()) {
            Ok(cmd)
                if cmd == Command::Heute
                    || cmd == Command::Morgen
                    || cmd == Command::Uebermorgen =>
            {
                let days_forward = match cmd {
                    Command::Heute => 0,
                    Command::Morgen => 1,
                    Command::Uebermorgen => 2,
                    _ => panic!(),
                };

                let now = Instant::now();
                registration_tx
                    .send(make_query_data(msg.chat.id.0))
                    .unwrap();

                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    if let Some(callback_id) = registration.4 {
                        _ = bot
                            .edit_message_reply_markup(msg.chat.id, MessageId(callback_id))
                            .await;
                    }
                    let text = build_meal_message(
                        days_forward,
                        registration.1,
                        #[cfg(feature = "mensimates")]
                        jwt_lock,
                    )
                    .await;
                    log::debug!("Build {:?} msg: {:.2?}", cmd, now.elapsed());
                    let now = Instant::now();

                    let keyboard = make_days_keyboard();
                    bot.send_message(msg.chat.id, text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?;

                    log::debug!("Send {:?} msg: {:.2?}", cmd, now.elapsed());

                    registration_tx
                        .send(make_query_data(msg.chat.id.0))
                        .unwrap();
                    let prev_reg = query_registration_rx.recv().await.unwrap().unwrap();
                    if let Some(callback_id) = prev_reg.4 {
                        _ = bot
                            .edit_message_reply_markup(msg.chat.id, MessageId(callback_id))
                            .await;
                    }

                    // send message callback id to store
                    let task = JobHandlerTask {
                        job_type: JobType::InsertCallbackMessageId,
                        chat_id: Some(msg.chat.id.0),
                        mensa_id: None,
                        hour: None,
                        minute: None,
                        callback_id: Some(msg.id.0),
                    };
                    update_db_row(&task).unwrap();
                    registration_tx.send(task).unwrap();
                } else {
                    // every user has a registration (starting the chat always calls /start)
                    // if this is none, it most likely means the DB was wiped)
                    // forcing reregistration is better than crashing the bot, no data will be overwritten anyways
                    // but copy pasting this everywhere is ugly
                    bot.send_message(msg.chat.id, NO_DB_MSG).await?;
                }
            }
            Ok(Command::Start) => {
                let keyboard = make_mensa_keyboard(mensen, false);
                bot.send_message(msg.chat.id, "Mensa auswählen:")
                    .reply_markup(keyboard)
                    .await?;
            }
            Ok(Command::Mensa) => {
                registration_tx
                    .send(make_query_data(msg.chat.id.0))
                    .unwrap();
                if query_registration_rx.recv().await.unwrap().is_some() {
                    let keyboard = make_mensa_keyboard(mensen, true);
                    bot.send_message(msg.chat.id, "Mensa auswählen:")
                        .reply_markup(keyboard)
                        .await?;
                } else {
                    bot.send_message(msg.chat.id, NO_DB_MSG).await?;
                }
            }
            Ok(Command::Subscribe) => {
                registration_tx
                    .send(make_query_data(msg.chat.id.0))
                    .unwrap();
                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    registration_tx
                        .send(make_query_data(msg.chat.id.0))
                        .unwrap();
                    let uuid = registration.0; //.unwrap().expect("try to operate on non-existing registration");

                    if uuid.is_some() {
                        bot.send_message(
                            msg.chat.id,
                            "Automatische Nachrichten sind schon aktiviert.",
                        )
                        .await?;
                    } else {
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
                            callback_id: None,
                        };

                        registration_tx.send(registration_job).unwrap();
                    }
                } else {
                    bot.send_message(msg.chat.id, NO_DB_MSG).await?;
                }
            }

            Ok(Command::Unsubscribe) => {
                registration_tx
                    .send(make_query_data(msg.chat.id.0))
                    .unwrap();
                if let Some(registration) = query_registration_rx.recv().await.unwrap() {
                    let uuid = registration.0;

                    if uuid.is_none() {
                        bot.send_message(
                            msg.chat.id,
                            "Automatische Nachrichten waren bereits deaktiviert.",
                        )
                        .await?;
                    } else {
                        bot.send_message(msg.chat.id, "Plan wird nicht mehr automatisch gesendet.")
                            .await?;

                        registration_tx
                            .send(JobHandlerTask {
                                job_type: JobType::Unregister,
                                chat_id: Some(msg.chat.id.0),
                                mensa_id: None,
                                hour: None,
                                minute: None,
                                callback_id: None,
                            })
                            .unwrap();
                    }
                } else {
                    bot.send_message(msg.chat.id, NO_DB_MSG).await?;
                }
            }
            Ok(Command::Uhrzeit) => {
                registration_tx
                    .send(make_query_data(msg.chat.id.0))
                    .unwrap();
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
                                callback_id: None,
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
                    bot.send_message(msg.chat.id, NO_DB_MSG).await?;
                }
            }
            Err(_) => {
                bot.send_message(msg.chat.id, "Ungültiger Befehl").await?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn make_mensa_keyboard(mensen: BTreeMap<&str, u8>, only_mensa_upd: bool) -> InlineKeyboardMarkup {
    let mut keyboard = Vec::new();

    // shitty
    for mensa in mensen {
        if only_mensa_upd {
            keyboard.push([InlineKeyboardButton::callback(
                mensa.0,
                format!("m_upd:{}", mensa.0),
            )]);
        } else {
            keyboard.push([InlineKeyboardButton::callback(
                mensa.0,
                format!("m_regist:{}", mensa.0),
            )]);
        }
    }
    InlineKeyboardMarkup::new(keyboard)
}

fn make_days_keyboard() -> InlineKeyboardMarkup {
    let keyboard = vec![vec![
        InlineKeyboardButton::callback("Heute", "day:0"),
        InlineKeyboardButton::callback("Morgen", "day:1"),
        InlineKeyboardButton::callback("Überm.", "day:2"),
    ]];
    InlineKeyboardMarkup::new(keyboard)
}

fn init_db_record(job_handler_task: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open("storage.sqlite")?;
    let mut stmt = conn
        .prepare_cached(
            "replace into registrations (chat_id, mensa_id, hour, minute)
            values (?1, ?2, ?3, ?4)",
        )
        .unwrap();

    stmt.execute(params![
        job_handler_task.chat_id,
        job_handler_task.mensa_id,
        job_handler_task.hour.unwrap(),
        job_handler_task.minute.unwrap()
    ])?;
    Ok(())
}

// fn update_db_row(chat_id: i64, mensa_id: Option<u8>, hour: Option<u32>, minute: Option<u32>) -> rusqlite::Result<()> {
fn update_db_row(data: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open("storage.sqlite")?;

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

    let mut upd_cb_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET last_callback_id = ?2
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

    if let Some(callback_id) = data.callback_id {
        upd_cb_stmt.execute([data.chat_id, Some(callback_id.into())])?;
    };

    Ok(())
}

fn task_db_kill_auto(chat_id: i64) -> rusqlite::Result<()> {
    let conn = Connection::open("storage.sqlite")?;
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
    let conn = Connection::open("storage.sqlite").unwrap();

    // ensure db table exists
    conn.prepare(
        "create table if not exists registrations (
        chat_id integer not null unique primary key,
        mensa_id integer not null,
        hour integer,
        minute integer,
        last_callback_id integer
        )",
    )
    .unwrap()
    .execute([])
    .unwrap();

    // // ensure db table exists
    // conn.prepare(
    //     "create table if not exists jwt (
    //         token text
    //     )",
    // )
    // .unwrap()
    // .execute([])
    // .unwrap();

    // you guessed it
    conn.prepare(
        "create table if not exists meals (
            mensa_and_date text,
            json_text
        )",
    )
    .unwrap()
    .execute([])
    .unwrap();

    let mut stmt = conn
        .prepare_cached(
            "SELECT chat_id, mensa_id, hour, minute, last_callback_id FROM registrations",
        )
        .unwrap();

    let job_iter = stmt
        .query_map([], |row| {
            Ok(JobHandlerTask {
                job_type: JobType::UpdateRegistration,
                chat_id: row.get(0)?,
                mensa_id: row.get(1)?,
                hour: row.get(2)?,
                minute: row.get(3)?,
                callback_id: row.get(4)?,
            })
        })
        .unwrap();

    for job in job_iter {
        tasks.push(job.unwrap())
    }

    tasks
}

async fn load_job(
    bot: Bot,
    sched: &JobScheduler,
    task: JobHandlerTask,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_rx: broadcast::Receiver<Option<RegistrationEntry>>,
    #[cfg(feature = "mensimates")] jwt_lock: Arc<RwLock<String>>,
) -> Option<Uuid> {
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
            #[cfg(feature = "mensimates")]
            let jwt_lock = jwt_lock.clone();
            let bot = bot.clone();
            let registration_tx = registration_tx.clone();
            let mut query_registration_rx = query_registration_rx.resubscribe();

            Box::pin(async move {
                let keyboard = make_days_keyboard();
                let msg = bot
                    .send_message(
                        ChatId(task.chat_id.unwrap()),
                        build_meal_message(
                            0,
                            task.mensa_id.unwrap(),
                            #[cfg(feature = "mensimates")]
                            jwt_lock,
                        )
                        .await,
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await
                    .unwrap();

                registration_tx
                    .send(make_query_data(task.chat_id.unwrap()))
                    .unwrap();

                let prev_reg = query_registration_rx.recv().await.unwrap().unwrap();
                if let Some(callback_id) = prev_reg.4 {
                    _ = bot
                        .edit_message_reply_markup(
                            ChatId(task.chat_id.unwrap()),
                            MessageId(callback_id),
                        )
                        .await;
                };

                // send message callback id to store
                let task = JobHandlerTask {
                    job_type: JobType::InsertCallbackMessageId,
                    chat_id: Some(task.chat_id.unwrap()),
                    mensa_id: None,
                    hour: None,
                    minute: None,
                    callback_id: Some(msg.id.0),
                };
                update_db_row(&task).unwrap();
                registration_tx.send(task).unwrap();
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
    #[cfg(feature = "mensimates")] jwt_lock: Arc<RwLock<String>>,
    #[cfg(not(feature = "mensimates"))] mensen: BTreeMap<&str, u8>,
) {
    let registr_tx_loadjob = registration_tx.clone();
    let tasks_from_db = get_all_tasks_db();
    let sched = JobScheduler::new().await.unwrap();

    // always update cache on startup
    cfg_if! {
        if #[cfg(not(feature = "mensimates"))] {
            // if default, start caching every 5 minutes and cache once at startup
            log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache...");
            let mensen_ids: Vec<u8> = mensen.values().copied().collect();
            update_cache(&mensen_ids).await;
            log::info!(target: "stuwe_telegram_rs::TaskSched", "Cache updated!");

            // run cache update every 5 minutes

            let cache_and_broadcast_job = Job::new_async("0 0/5 * * * *", move |_uuid, mut _l| {
                log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache");

                let registration_tx = registration_tx.clone();

                let mensen_ids = mensen_ids.clone();
                Box::pin(async move {
                    let registration_tx = registration_tx.clone();
                    let today_changed_mensen = update_cache(&mensen_ids).await;

                    for mensa in today_changed_mensen {
                        registration_tx
                            .send(JobHandlerTask {
                                job_type: JobType::BroadcastUpdate,
                                chat_id: None,
                                mensa_id: Some(mensa),
                                hour: None,
                                minute: None,
                                callback_id: None,
                            })
                            .unwrap();
                    }
                })
            })
            .unwrap();
            sched.add(cache_and_broadcast_job).await.unwrap();
        } else {
            // if mensimates, create job to reload token every minute
            // reload now
            {
                let jwt_lock = jwt_lock.clone();
                let mut wr = jwt_lock.write_owned().await;
                *wr = get_jwt_token().await;
            }
            {
                let jwt_lock = jwt_lock.clone();
                let jwt_job = Job::new_async("0 * * * * *", move |_uuid, mut _l| {
                    let jwt_lock = jwt_lock.clone();
                    Box::pin(async move {
                        let mut wr = jwt_lock.write_owned().await;
                        *wr = get_jwt_token().await;
                    })
                })
                .unwrap();
                sched.add(jwt_job).await.unwrap();
            }
        }
    }

    for task in tasks_from_db {
        let bot = bot.clone();
        let registration_tx = registr_tx_loadjob.clone();

        let uuid = load_job(
            bot,
            &sched,
            task.clone(),
            registration_tx,
            query_registration_tx.subscribe(),
            #[cfg(feature = "mensimates")]
            jwt_lock.clone(),
        )
        .await;
        loaded_user_data.insert(
            task.chat_id.unwrap(),
            (
                uuid,
                task.mensa_id.unwrap(),
                task.hour,
                task.minute,
                task.callback_id,
            ),
        );
    }

    // start scheduler (non blocking)
    sched.start().await.unwrap();

    log::info!(target: "stuwe_telegram_rs::TaskSched", "Ready.");

    // receive job update msg (register/unregister/check existence)
    while let Ok(job_handler_task) = job_rx.recv().await {
        match job_handler_task.job_type {
            JobType::Register => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Register: {} for Mensa {}", &job_handler_task.chat_id.unwrap(), &job_handler_task.mensa_id.unwrap());
                // creates a new row, or replaces every col with new values
                init_db_record(&job_handler_task).unwrap();

                let previous_callback_msg = if let Some(previous_registration) =
                    loaded_user_data.get(&job_handler_task.chat_id.unwrap())
                {
                    if let Some(uuid) = previous_registration.0 {
                        sched.context.job_delete_tx.send(uuid).unwrap();
                    };

                    previous_registration.4
                } else {
                    None
                };

                // get uuid (here guaranteed to be Some() since default is registration with job)
                let new_uuid = load_job(
                    bot.clone(),
                    &sched,
                    job_handler_task.clone(),
                    registr_tx_loadjob.clone(),
                    query_registration_tx.subscribe(),
                    #[cfg(feature = "mensimates")]
                    jwt_lock.clone(),
                )
                .await;

                // insert new job uuid
                loaded_user_data.insert(
                    job_handler_task.chat_id.unwrap(),
                    (
                        new_uuid,
                        job_handler_task.mensa_id.unwrap(),
                        job_handler_task.hour,
                        job_handler_task.minute,
                        previous_callback_msg,
                    ),
                );
            }
            JobType::UpdateRegistration => {
                if job_handler_task.mensa_id.is_some() {
                    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{} changed Mensa", job_handler_task.chat_id.unwrap());
                }
                if job_handler_task.hour.is_some() {
                    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{} changed time", job_handler_task.chat_id.unwrap());
                }

                if let Some(previous_registration) =
                    loaded_user_data.get(&job_handler_task.chat_id.unwrap())
                {
                    let mensa_id = if job_handler_task.mensa_id.is_some() {
                        job_handler_task.mensa_id.unwrap()
                    } else {
                        previous_registration.1
                    };

                    let new_job_task = JobHandlerTask {
                        job_type: JobType::UpdateRegistration,
                        chat_id: job_handler_task.chat_id,
                        mensa_id: Some(mensa_id),
                        hour: job_handler_task.hour,
                        minute: job_handler_task.minute,
                        callback_id: None,
                    };

                    let new_uuid =
                        // new time was passed -> unload old job, load new
                        if job_handler_task.hour.is_some() || job_handler_task.minute.is_some() {
                            if let Some(uuid) = previous_registration.0 {
                                // unload old job if exists
                                sched.context.job_delete_tx.send(uuid).unwrap();
                            }
                            // load new job, return uuid
                            load_job(
                                bot.clone(),
                                &sched,
                                new_job_task.clone(),
                                registr_tx_loadjob.clone(),
                                query_registration_tx.subscribe(),
                                #[cfg(feature = "mensimates")]
                                jwt_lock.clone()
                            ).await
                        } else {
                            // no new time was set -> return old job uuid
                            previous_registration.0
                        };

                    let previous_callback_id = if previous_registration.4.is_some() {
                        previous_registration.4
                    } else {
                        None
                    };

                    loaded_user_data.insert(
                        job_handler_task.chat_id.unwrap(),
                        (
                            new_uuid,
                            mensa_id,
                            job_handler_task.hour,
                            job_handler_task.minute,
                            previous_callback_id,
                        ),
                    );

                    // update any values that are to be changed, aka are Some()
                    update_db_row(&job_handler_task).unwrap();
                } else {
                    log::error!("Tried to update non-existent job");
                    bot.send_message(
                        ChatId(job_handler_task.chat_id.unwrap()),
                        "Bitte zuerst /start ausführen",
                    )
                    .await
                    .unwrap();
                }
            }

            JobType::Unregister => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Unregister: {}", &job_handler_task.chat_id.unwrap());

                // unregister is only invoked if existence of job is guaranteed
                let previous_registration = loaded_user_data
                    .get(&job_handler_task.chat_id.unwrap())
                    .unwrap();

                // unload old job
                sched
                    .context
                    .job_delete_tx
                    .send(previous_registration.0.unwrap())
                    .unwrap();

                let previous_callback_id = if previous_registration.4.is_some() {
                    previous_registration.4
                } else {
                    None
                };

                // kill uuid from this thing
                loaded_user_data.insert(
                    job_handler_task.chat_id.unwrap(),
                    (
                        None,
                        previous_registration.1,
                        None,
                        None,
                        previous_callback_id,
                    ),
                );

                // delete from db
                task_db_kill_auto(job_handler_task.chat_id.unwrap()).unwrap();
            }

            JobType::QueryRegistration => {
                // check if uuid exists
                let uuid_opt = loaded_user_data.get(&job_handler_task.chat_id.unwrap());
                if uuid_opt.is_none() {
                    log::error!(target: "stuwe_telegram_rs::TS::Jobs", "chat_id {} sent a command, but is not in DB", &job_handler_task.chat_id.unwrap());
                }

                query_registration_tx.send(uuid_opt.copied()).unwrap();
            }

            #[cfg(not(feature = "mensimates"))]
            JobType::BroadcastUpdate => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "TodayMeals changed @Mensa {}", &job_handler_task.mensa_id.unwrap());
                for (chat_id, registration_data) in &loaded_user_data {
                    let mensa_id = registration_data.1;

                    let now = chrono::Local::now();

                    if let (Some(job_hour), Some(job_minute)) =
                        (registration_data.2, registration_data.3)
                    {
                        // filter only registrations with same mensa id as update
                        if mensa_id == job_handler_task.mensa_id.unwrap()
                        // only push updates after first auto msg: job hour has to be sooner, or same hour but minute has to be sooner
                        && (job_hour < now.hour() || job_hour == now.hour() && job_minute <= now.minute())
                        {
                            log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Sent update to {}", chat_id);

                            // delete old callback buttons (heute/morgen/ueb)
                            if let Some(callback_id) = registration_data.4 {
                                _ = bot
                                    .edit_message_reply_markup(
                                        ChatId(*chat_id),
                                        MessageId(callback_id),
                                    )
                                    .await;
                            };
                            bot.send_message(
                                ChatId(*chat_id),
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

            JobType::InsertCallbackMessageId => {
                let mut regist = *loaded_user_data
                    .get(&job_handler_task.chat_id.unwrap())
                    .unwrap();

                regist.4 = job_handler_task.callback_id;

                loaded_user_data.insert(job_handler_task.chat_id.unwrap(), regist);
            }
        }
    }
}

#[derive(BotCommands, Clone, Debug, PartialEq)]
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
        #[dynamic]
        static RE: Regex = Regex::new("^([01]?[0-9]|2[0-3]):([0-5][0-9])").unwrap();
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
