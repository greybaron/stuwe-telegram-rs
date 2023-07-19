
mod data_types;
mod stuwe_parser;
use log::log_enabled;
use stuwe_parser::{build_meal_message, update_cache};

use data_types::{JobHandlerTask, JobType, TimeParseError};

use chrono::{Timelike};
use regex::Regex;
use rusqlite::{params, Connection, Result, types::Null as SqlNull};
use std::{
    collections::{hash_map::Keys, HashMap},
    env,
    error::Error, time::{Duration, Instant}, sync::Arc,
};
use teloxide::{
    payloads::AnswerCallbackQuery,
    prelude::*,
    types::{
        InlineKeyboardButton, InlineKeyboardMarkup, InlineQueryResultArticle, InputMessageContent,
        InputMessageContentText, Me, ParseMode,
    },
    utils::{command::BotCommands, markdown},
};
use tokio::{sync::{broadcast, mpsc, Notify}, time::sleep};
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use teloxide::types::{ButtonRequest, KeyboardButton, KeyboardMarkup};

use teloxide::{dispatching::dialogue::InMemStorage, prelude::*};

type MyDialogue = Dialogue<State, InMemStorage<State>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Default)]
pub enum State {
    #[default]
    Start,
    ReceiveFullName,
    ReceiveAge {
        full_name: String,
    },
    ReceiveLocation {
        full_name: String,
        age: u8,
    },
}

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

    let (job_tx, job_rx): (
        broadcast::Sender<JobHandlerTask>,
        broadcast::Receiver<JobHandlerTask>,
    ) = broadcast::channel(10);
    let job_tx2 = job_tx.clone();

    let (query_registration_tx, query_registration_rx): (
        broadcast::Sender<Option<JobHandlerTask>>,
        broadcast::Receiver<Option<JobHandlerTask>>,
    ) = broadcast::channel(10);
    let query_registration_tx_tasksched = query_registration_tx.clone();

    let bot = Bot::from_env();
    let c_bot = bot.clone();

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(message_handler))
        .branch(Update::filter_callback_query().endpoint(callback_handler));

    // chat_id -> opt(job_uuid), mensa_id
    // every user has a mensa_id, but only users with auto send have a job_uuid
    let loaded_user_data: HashMap<i64, (Option<Uuid>, u8)> = HashMap::new();
    
    // let (query_user_data_tx, query_user_data_rx): (mpsc::Sender<_>, mpsc::Receiver<_>) = mpsc::channel(10);
    let (user_data_tx, user_data_rx): (broadcast::Sender<(Option<Uuid>, u8)>, broadcast::Receiver<(Option<Uuid>, u8)>) = broadcast::channel(10);

    tokio::spawn(async move {
        log::info!("Starting task scheduler...");
        init_task_scheduler(c_bot, job_tx2, job_rx, query_registration_tx, loaded_user_data, mensen_c, user_data_rx, user_data_tx).await;
    });

    
    // passing a receiver doesnt work, so sending query_registration_tx and resubscribing to get rx
    let command_handler_deps = dptree::deps![mensen, job_tx, query_registration_tx_tasksched]; //, job_tx, job_query_rx.resubscribe()];
    Dispatcher::builder(bot, handler)
        .dependencies(command_handler_deps)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    // Ok(())

    // Dispatcher::builder(
    //     bot,
    //     Update::filter_message()
    //         .enter_dialogue::<Message, InMemStorage<State>, State>()
    //         .branch(dptree::case![State::Start].endpoint(start))
    // )
    // .dependencies(dptree::deps![InMemStorage::<State>::new()])
    // .enable_ctrlc_handler()
    // .build()
    // .dispatch()
    // .await;


    // Command::repl(bot, move |bot, msg, cmd| {
    //     // let mensen = mensen.clone();
    //     answer(bot, msg, cmd, mensen_c.clone(), job_tx2.clone(), job_exists_rx.resubscribe())
    // })
    // .await;
}

async fn callback_handler(
    bot: Bot,
    q: CallbackQuery,
    mensen: HashMap<&str, u8>,
    job_tx: broadcast::Sender<JobHandlerTask>,

) -> Result<(), Box<dyn Error + Send + Sync>> {
    // let test = q.message.unwrap().text();
    
    if let Some(mensa_name) = q.data {
        let mut job_rx = job_tx.subscribe();

        // Tell telegram that we've seen this query, to remove 🕑 icons from the
        // clients. You could also use `answer_callback_query`'s optional
        // parameters to tweak what happens on the client side.
        bot.answer_callback_query(q.id).await?;

        // Edit text of the message to which the buttons were attached
        if let Some(Message { id, chat, .. }) = q.message {
            let new_registration_data =
            // fuck no
            if mensa_name.starts_with('#') {
                let mensa_name = &mensa_name[1..];
                bot.edit_message_text(chat.id, id, format!("Gewählte Mensa: {}", markdown::bold(&mensa_name))).parse_mode(ParseMode::MarkdownV2).await.unwrap();

                JobHandlerTask {
                    job_type: JobType::UpdateRegistration,
                    mensa_id: Some(*mensen.get(mensa_name).unwrap()),
                    chatid: Some(chat.id.0),
                    hour: None,
                    minute: None,
                }
            }
            else {
                let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";
                bot.edit_message_text(chat.id, id, Command::descriptions().to_string() + subtext)
                .await?;

                bot.send_message(chat.id, format!("Plan der {} wird ab jetzt automatisch an Wochentagen *06:00 Uhr* gesendet\\.\n\nÄndern mit /changetime [Zeit]", markdown::bold(&mensa_name)))
                .parse_mode(ParseMode::MarkdownV2).await?;

                JobHandlerTask {
                    job_type: JobType::Register,
                    mensa_id: Some(*mensen.get(mensa_name.as_str()).unwrap()),
                    chatid: Some(chat.id.0),
                    hour: Some(6),
                    minute: Some(0),
                }
            };

            job_tx.send(new_registration_data).unwrap();
        }
    }

    Ok(())
}

async fn message_handler(
    bot: Bot,
    msg: Message,
    me: Me,
    mensen: HashMap<&str, u8>,
    job_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<JobHandlerTask>>

) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut query_registration_rx = query_registration_tx.subscribe();

    if let Some(text) = msg.text() {
        match BotCommands::parse(text, me.username()) {
            Ok(Command::Heute) => {
                let mensa_id = get_task_from_db(msg.chat.id.0).unwrap().mensa_id.unwrap();
                bot.send_message(msg.chat.id, build_meal_message(0, mensa_id).await)
                    .parse_mode(ParseMode::MarkdownV2).await?;
            }
            Ok(Command::Morgen) => {
                let mensa_id = get_task_from_db(msg.chat.id.0).unwrap().mensa_id.unwrap();
                bot.send_message(msg.chat.id, build_meal_message(1, mensa_id).await)
                .parse_mode(ParseMode::MarkdownV2).await?;

            }
            Ok(Command::Uebermorgen) => {
                let mensa_id = get_task_from_db(msg.chat.id.0).unwrap().mensa_id.unwrap();
                bot.send_message(msg.chat.id, build_meal_message(2, mensa_id).await)
                .parse_mode(ParseMode::MarkdownV2).await?;
        }
            Ok(Command::Start) => {
                let keyboard = make_mensa_keyboard(mensen, false);
                bot.send_message(msg.chat.id, "Mensa auswählen:")
                    .reply_markup(keyboard)
                    .await?;
            }
            Ok(Command::Mensa) => {
                let keyboard = make_mensa_keyboard(mensen, true);
                bot.send_message(msg.chat.id, "Mensa auswählen:")
                    .reply_markup(keyboard)
                    .await?;
            }
            Ok(Command::Subscribe) => {
                let check_exists_task = JobHandlerTask {
                    job_type: JobType::GetExistingJobData,
                    mensa_id: None,
                    chatid: Some(msg.chat.id.0),
                    hour: None,
                    minute: None,
                };
                job_tx.send(check_exists_task).unwrap();
                let current_registration = query_registration_rx.recv().await.unwrap().expect("try to operate on non-existing registration");

                if current_registration.hour.is_some() {
                    bot.send_message(
                        msg.chat.id,
                        "Automatische Nachrichten sind schon aktiviert.",
                    )
                    .await?;
                }
                else {
                    let new_msg_job = JobHandlerTask {
                        job_type: JobType::UpdateRegistration,
                        mensa_id: None,
                        chatid: current_registration.chatid,
                        hour: Some(6),
                        minute: Some(0),
                    };

                    job_tx.send(new_msg_job).unwrap();

                    bot.send_message(
                        msg.chat.id,
                        "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /changetime [Zeit]",
                    )
                    .await?;
                }
            }

            Ok(Command::Unsubscribe) => {
                let check_exists_task = JobHandlerTask {
                    job_type: JobType::GetExistingJobData,
                    mensa_id: None,
                    chatid: Some(msg.chat.id.0),
                    hour: None,
                    minute: None,
                };
                job_tx.send(check_exists_task).unwrap();
                let current_registration = query_registration_rx.recv().await.unwrap().expect("try to operate on non-existing registration");

                if current_registration.hour.is_none() {
                    println!("curr reg hour is none");
                    bot.send_message(
                        msg.chat.id,
                        "Automatische Nachrichten waren bereits deaktiviert.",
                    )
                    .await?;

                } else {
                    println!("curr reg hour is some: {}", current_registration.hour.unwrap());
                    job_tx.send(JobHandlerTask {
                        job_type: JobType::Unregister,
                        mensa_id: None,
                        chatid: Some(msg.chat.id.0),
                        hour: None,
                        minute: None,
                    }).unwrap();
                    bot.send_message(
                        msg.chat.id,
                        "Plan wird nicht mehr automatisch gesendet.",
                    )
                    .await?;
                }
               
                
                // if mensa_id_rx.recv().await {
                //     bot.send_message(
                //         msg.chat.id,
                //         "Automatische Nachrichten waren bereits deaktiviert.",
                //     )
                //     .await?
                // } else {
                //     let kill_msg = JobHandlerTask {
                //         job_type: JobType::Unregister,
                //         mensa_id: None,
                //         chatid: Some(msg.chat.id.0),
                //         hour: None,
                //         minute: None,
                //     };

                //     job_tx.send(kill_msg).unwrap();
                //     bot.send_message(msg.chat.id, "Plan wird nicht mehr automatisch gesendet.")
                //         .await?
                // }
            }
            Ok(Command::Changetime) => {
                match parse_time(msg.text().unwrap()) {

                    Ok((hour, minute)) => {
                        let new_msg_job = JobHandlerTask {
                            job_type: JobType::UpdateRegistration,
                            mensa_id: None,
                            chatid: Some(msg.chat.id.0),
                            hour: Some(hour),
                            minute: Some(minute),
                        };
        
                        // update_db_row(&new_msg_job).unwrap();

                        job_tx.send(new_msg_job).unwrap();
                        bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/unsubscribe zum Deaktivieren", hour, minute)).await?
                    },
                    Err(TimeParseError::NoTimePassed) => {
                        bot.send_message(msg.chat.id, "Bitte Zeit angeben\n/changetime [Zeit]")
                            .await?
                    },
                    Err(TimeParseError::InvalidTimePassed) => {
                        bot.send_message(msg.chat.id, "Eingegebene Zeit ist ungültig.")
                            .await?
                    }
                };
            },
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
            keyboard.push([InlineKeyboardButton::callback(mensa.0, format!("#{}" ,mensa.0))]);
        } else {
            keyboard.push([InlineKeyboardButton::callback(mensa.0, mensa.0)]);
        }
        
    }

    InlineKeyboardMarkup::new(keyboard)
}

fn init_db_record(msg_job: &JobHandlerTask) -> rusqlite::Result<()> {
    // let conn = Connection::open("../../jobs.db")?;
    //
    //
    //
    //
    //
    //
    //
    //
    //
    //
    //
    let conn = Connection::open("jobs.db")?;
    let mut stmt = conn
        .prepare_cached(
            "replace into jobs (chatid, mensaid, hour, minute)
            values (?1, ?2, ?3, ?4)",
        )
        .unwrap();

        stmt.execute(params![msg_job.chatid, msg_job.mensa_id, msg_job.hour.unwrap(), msg_job.minute.unwrap()])?;
    Ok(())
}


// fn update_db_row(chatid: i64, mensa_id: Option<u8>, hour: Option<u32>, minute: Option<u32>) -> rusqlite::Result<()> {
fn update_db_row(data: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open("jobs.db")?;
    let mut update_mensa_stmt = conn
        .prepare_cached(
            "UPDATE jobs
            SET mensaid = ?2
            WHERE chatid = ?1",
        )
        .unwrap();

    let mut upd_hour_stmt = conn
        .prepare_cached(
            "UPDATE jobs
            SET hour = ?2
            WHERE chatid = ?1",
        )
        .unwrap();

        let mut upd_min_stmt = conn
        .prepare_cached(
            "UPDATE jobs
            SET minute = ?2
            WHERE chatid = ?1",
        )
        .unwrap();

    if let Some(mensa_id) = data.mensa_id {
        update_mensa_stmt.execute(params![data.chatid, mensa_id])?;
    };

    if let Some(hour) = data.hour {
        upd_hour_stmt.execute(params![data.chatid, hour])?;
    };

    if let Some(minute) = data.minute {
        upd_min_stmt.execute(params![data.chatid, minute])?;
    };
    
    Ok(())
}



fn task_db_kill_auto(chatid: i64) -> rusqlite::Result<()> {
    println!("kill auto for cid: {}", chatid);

    let conn = Connection::open("jobs.db")?;
    let mut stmt = conn
        .prepare_cached(
            "UPDATE jobs
            SET hour = NULL, minute = NULL
            WHERE chatid = ?1")
        .unwrap();

    stmt.execute(params![chatid])?;

    Ok(())
}

fn get_task_from_db(chatid: i64) -> Option<JobHandlerTask> {
    let now = Instant::now();
    println!("chatid: {}", chatid);
    let conn = Connection::open("jobs.db").unwrap();
    let mut stmt = conn.prepare_cached("SELECT chatid, mensaid, hour, minute FROM jobs WHERE chatid = ?1").unwrap();

    let test = Some(stmt.query_row([chatid], |row| {
        Ok(JobHandlerTask {
            job_type: JobType::UpdateRegistration,
            chatid: row.get(0)?,
            mensa_id: row.get(1)?,
            hour: row.get(2)?,
            minute: row.get(3)?,
        })
    }).unwrap());

    println!("get_task_from_db took: {:?}", now.elapsed());
    test
}

fn get_all_tasks_db() -> Vec<JobHandlerTask> {
    println!("get_all_tasks_db");
    let mut tasks: Vec<JobHandlerTask> = Vec::new();
    let conn = Connection::open("jobs.db").unwrap();
    let mut stmt = conn
        .prepare_cached(
            "create table if not exists jobs (
        chatid integer not null unique primary key,
        mensaid integer not null,
        hour integer,
        minute integer
    )",
        )
        .unwrap();

    stmt.execute(()).unwrap();

    let mut stmt = conn.prepare_cached("SELECT chatid, mensaid, hour, minute FROM jobs").unwrap();

    let job_iter = stmt.query_map([], |row| {
        Ok(JobHandlerTask {
            job_type: JobType::UpdateRegistration,
            chatid: row.get(0)?,
            mensa_id: row.get(1)?,
            hour: row.get(2)?,
            minute: row.get(3)?,
        })
    }).unwrap();

    for job in job_iter {
        tasks.push(job.unwrap())
    }

    tasks
}

async fn load_job(bot: Bot, sched: &JobScheduler, task: JobHandlerTask) -> Option<Uuid> {
    if task.hour.is_none() {
        return None;
    }

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
                    ChatId(task.chatid.unwrap()),
                    build_meal_message(0, 140).await,
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
    job_tx: broadcast::Sender<JobHandlerTask>,
    mut job_rx: broadcast::Receiver<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<JobHandlerTask>>,
    mut loaded_user_data: HashMap<i64, (Option<Uuid>, u8)>,
    mensen: HashMap<&str, u8>,
    user_data_rx: broadcast::Receiver<(Option<Uuid>, u8)>,
    user_data_tx: broadcast::Sender<(Option<Uuid>, u8)>,
) {
    let tasks_from_db = get_all_tasks_db();
    let sched = JobScheduler::new().await.unwrap();

    let mensen_ids = mensen.values().map(|x| *x).collect::<Vec<u8>>();

    log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache...");
    // always update cache on startup
    update_cache(&mensen_ids).await;

    log::info!(target: "stuwe_telegram_rs::TaskSched", "Cache updated!");

    // run cache update every 5 minutes
    let cache_job = Job::new_async("0 1/5 * * * *", move |_uuid, mut _l| {
        let job_tx = job_tx.clone();
        
        let mensen_ids = mensen_ids.clone();

        Box::pin(async move {
            log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache");

            let today_changed_mensen = update_cache(&mensen_ids).await;

            for mensa in today_changed_mensen {
                log::info!(target: "stuwe_telegram_rs::TaskSched", "Broadcasting update for today @ Mensa {}", mensa);

                job_tx
                    .send(JobHandlerTask {
                        job_type: JobType::BroadcastUpdate,
                        mensa_id: Some(mensa),
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

    for task in tasks_from_db {
        let bot = bot.clone();


        let uuid = load_job(bot, &sched, task.clone()).await;
        loaded_user_data.insert(task.chatid.unwrap(), (uuid, task.mensa_id.unwrap()));
    }

    // start scheduler (non blocking)
    sched.start().await.unwrap();

    // tokio::spawn(async move {
    //     loop {
    //         notify.notified().await;


    //     }
    // });

    log::info!(target: "stuwe_telegram_rs::TaskSched", "Ready.");

    // receive job update msg (register/unregister/check existence)
    while let Ok(msg_job) = job_rx.recv().await {
        match msg_job.job_type {
            JobType::Register => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Register: {} for Mensa {}", &msg_job.chatid.unwrap(), &msg_job.mensa_id.unwrap());
                // creates a new row, or replaces every col with new values
                init_db_record(&msg_job).unwrap();

                // let test  = loaded_user_data.get(&msg_job.chatid.unwrap());
                if let Some(previous_registration) = loaded_user_data.get(&msg_job.chatid.unwrap()) {
                    if let Some(uuid) = previous_registration.0 {
                        sched.context.job_delete_tx.send(uuid).unwrap();
                    }
                };

                // get uuid (here guaranteed to be Some() since default is registration with job)
                let new_uuid = load_job(bot.clone(), &sched, msg_job.clone()).await;

                // insert new job uuid
                loaded_user_data.insert(msg_job.chatid.unwrap(), (new_uuid, msg_job.mensa_id.unwrap()));
            }
            JobType::UpdateRegistration => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{}: Changed: {} {}",
                msg_job.chatid.unwrap(),
                if msg_job.mensa_id.is_some() {"Mensa-ID"} else {""},
                if msg_job.hour.is_some() {"Time"} else {""},    
            );

                let previous_registration = loaded_user_data.get(&msg_job.chatid.unwrap()).unwrap();

                let new_uuid =
                if msg_job.hour.is_some() || msg_job.minute.is_some() {
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

                loaded_user_data.insert(msg_job.chatid.unwrap(), (new_uuid, mensa_id));

                // update any values that are to be changed, aka are Some()
                update_db_row(&msg_job).unwrap();
            }

            JobType::Unregister => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Unregister: {}", &msg_job.chatid.unwrap());

                // unregister is only invoked if existence of job is guaranteed
                let previous_registration = loaded_user_data.get(&msg_job.chatid.unwrap()).unwrap();

                // unload old job
                sched.context.job_delete_tx.send(previous_registration.0.unwrap()).unwrap();

                // kill uuid from this thing 
                loaded_user_data.insert(msg_job.chatid.unwrap(), (None, previous_registration.1));

                // delete from db
                task_db_kill_auto(msg_job.chatid.unwrap()).unwrap();
            }

            JobType::GetExistingJobData => {
                // check if uuid exists
                let uuid_opt = loaded_user_data.get(&msg_job.chatid.unwrap()).unwrap().0;

                if uuid_opt.is_some() {
                    // dumm
                    query_registration_tx.send(Some(JobHandlerTask {
                        job_type: JobType::GetExistingJobData,
                        mensa_id: None,
                        chatid: None,
                        hour: Some(0),
                        minute: None,
                    })).unwrap();
                } else {
                    // please refactor
                    query_registration_tx.send(Some(JobHandlerTask {
                        job_type: JobType::GetExistingJobData,
                        mensa_id: None,
                        chatid: None,
                        hour: None,
                        minute: None,
                    })).unwrap();
                }
                

                // let uuid_and_mensaid = loaded_user_data.get(&msg_job.chatid.unwrap()).unwrap();
                // if let Some(_) = uuid_and_mensaid.0 {
                    
                // }


                // query_registration_tx.send(get_task_from_db(msg_job.chatid.unwrap())).unwrap();
                
                // mensa_id_tx.send(msg_job.mensa_id.unwrap()).unwrap();
                // job_exists_tx
                //     .send(loaded_user_data.get(&msg_job.chatid.unwrap()).is_some())
                //     .unwrap();
            }
            JobType::BroadcastUpdate => {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Broadcast Update");
                for registration in &loaded_user_data {
                    bot.send_message(
                        ChatId(*registration.0),
                        format!(
                            "{}\n{}",
                            &markdown::bold(&markdown::underline("Planänderung:")),
                            build_meal_message(0, 140).await
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
    Mensa,
    #[command(description = "off")]
    Start,
}

// async fn answer(
//     bot: Bot,
//     msg: Message,
//     cmd: Command,
//     mensen: HashMap<&str, u8>,
//     job_tx: broadcast::Sender<JobHandlerTask>,
//     mut job_exists_rx: broadcast::Receiver<bool>,
// ) -> ResponseResult<()> {
//     match cmd {
//         Command::Start => {

//             let subtext = "\n\nWenn /heute oder /morgen kein Wochentag ist, wird der Plan für Montag angezeigt.";

//             bot.send_message(msg.chat.id, Command::descriptions().to_string() + subtext)
//                 .await?;

//             let check_exists_task = JobHandlerTask {
//                 job_type: JobType::GetExistingJobData,
//                 mensa_id: None,
//                 chatid: Some(msg.chat.id.0),
//                 hour: None,
//                 minute: None,
//             };

//             job_tx.send(check_exists_task).unwrap();

//             if !job_exists_rx.recv().await.unwrap() {
//                 let first_msg_job = JobHandlerTask {
//                     job_type: JobType::UpdateRegistration,
//                     mensa_id: None,
//                     chatid: Some(msg.chat.id.0),
//                     hour: Some(6),
//                     minute: Some(0),
//                 };

//                 job_tx.send(first_msg_job).unwrap();
//                 bot.send_message(
//                     msg.chat.id,
//                     "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /changetime [Zeit]",
//                 )
//                 .await?
//             } else {
//                 bot.send_message(msg.chat.id, "Automatische Nachrichten sind aktiviert.")
//                     .await?
//             }
//         }
//         Command::Heute => {
//             let text = build_meal_message(0, 140).await;
//             bot.send_message(msg.chat.id, text)
//                 .parse_mode(ParseMode::MarkdownV2)
//                 .await?
//         }
//         Command::Morgen => {
//             let text = build_meal_message(1, 140).await;
//             bot.send_message(msg.chat.id, text)
//                 .parse_mode(ParseMode::MarkdownV2)
//                 .await?
//         }
//         Command::Uebermorgen => {
//             let text = build_meal_message(2, 140).await;
//             bot.send_message(msg.chat.id, text)
//                 .parse_mode(ParseMode::MarkdownV2)
//                 .await?
//         }
//         Command::Subscribe => {
//             let check_exists_task = JobHandlerTask {
//                 job_type: JobType::GetExistingJobData,
//                 chatid: Some(msg.chat.id.0),
//                 mensa_id: None,
//                 hour: None,
//                 minute: None,
//             };
//             job_tx.send(check_exists_task).unwrap();

//             if job_exists_rx.recv().await.unwrap() {
//                 bot.send_message(
//                     msg.chat.id,
//                     "Automatische Nachrichten sind schon aktiviert.",
//                 )
//                 .await?
//             } else {
//                 let new_msg_job = JobHandlerTask {
//                     job_type: JobType::UpdateRegistration,
//                     mensa_id: None,
//                     chatid: Some(msg.chat.id.0),
//                     hour: Some(6),
//                     minute: Some(0),
//                 };
//                 job_tx.send(new_msg_job).unwrap();
//                 bot.send_message(
//                         msg.chat.id,
//                         "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /changetime [Zeit]",
//                     )
//                     .await?
//             }
//         }

//         Command::Unsubscribe => {
//             let check_exists_task = JobHandlerTask {
//                 job_type: JobType::GetExistingJobData,
//                 mensa_id: None,
//                 chatid: Some(msg.chat.id.0),
//                 hour: None,
//                 minute: None,
//             };

//             job_tx.send(check_exists_task).unwrap();

//             if !job_exists_rx.recv().await.unwrap() {
//                 bot.send_message(
//                     msg.chat.id,
//                     "Automatische Nachrichten waren bereits deaktiviert.",
//                 )
//                 .await?
//             } else {
//                 let kill_msg = JobHandlerTask {
//                     job_type: JobType::Unregister,
//                     mensa_id: None,
//                     chatid: Some(msg.chat.id.0),
//                     hour: None,
//                     minute: None,
//                 };

//                 job_tx.send(kill_msg).unwrap();
//                 bot.send_message(msg.chat.id, "Plan wird nicht mehr automatisch gesendet.")
//                     .await?
//             }
//         }

//         Command::Changetime => match parse_time(msg.text().unwrap()) {
//             Ok((hour, minute)) => {
//                 let new_msg_job = JobHandlerTask {
//                     job_type: JobType::UpdateRegistration,
//                     mensa_id: None,
//                     chatid: Some(msg.chat.id.0),
//                     hour: Some(hour),
//                     minute: Some(minute),
//                 };

//                 init_db_record(&new_msg_job).unwrap();
//                 job_tx.send(new_msg_job).unwrap();
//                 bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/unsubscribe zum Deaktivieren", hour, minute)).await?
//             }
//             Err(TimeParseError::NoTimePassed) => {
//                 bot.send_message(msg.chat.id, "Bitte Zeit angeben\n/changetime [Zeit]")
//                     .await?
//             }
//             Err(TimeParseError::InvalidTimePassed) => {
//                 bot.send_message(msg.chat.id, "Eingegebene Zeit ist ungültig.")
//                     .await?
//             }
//         },
//     };
//     Ok(())
// }

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
