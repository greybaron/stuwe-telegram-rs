use stuwe_telegram_rs::data_backend::mm_parser::get_jwt_token;
use stuwe_telegram_rs::data_types::{
    Backend, Command, DialogueState, HandlerResult, JobHandlerTask, JobHandlerTaskType, JobType,
    QueryRegistrationType, RegistrationEntry, MENSEN, MM_DB, NO_DB_MSG,
};
use stuwe_telegram_rs::db_operations::{
    get_all_tasks_db, init_db_record, task_db_kill_auto, update_db_row,
};
use stuwe_telegram_rs::shared_main::{
    build_meal_message_dispatcher, callback_handler, load_job, logger_init, make_days_keyboard,
    make_mensa_keyboard, make_query_data, process_time_reply, start_time_dialogue,
};

use log::log_enabled;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Instant,
};
use teloxide::{
    dispatching::{
        dialogue::{self, InMemStorage},
        UpdateHandler,
    },
    prelude::*,
    types::{MessageId, ParseMode},
};
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler};

#[tokio::main]
async fn main() {
    logger_init(module_path!());

    log::info!("Starting command bot...");

    let bot = Bot::from_env();
    let mensen = BTreeMap::from(MENSEN);
    let jwt_lock: Arc<RwLock<String>> = Arc::new(RwLock::new(String::new()));

    if !log_enabled!(log::Level::Debug) {
        log::info!("Set env variable 'RUST_LOG=debug' for performance metrics");
    }

    let (registration_tx, job_rx): JobHandlerTaskType = broadcast::channel(10);

    let (query_registration_tx, _): QueryRegistrationType = broadcast::channel(10);

    // every user has a mensa_id, but only users with auto send have a job_uuid inside RegistrEntry
    let loaded_user_data: HashMap<i64, RegistrationEntry> = HashMap::new();
    {
        let bot = bot.clone();
        // there is effectively only one tx and rx, however since rx cant be passed as dptree dep (?!),
        // tx has to be cloned and passed to both (inside command_handler it will be resubscribed to rx)
        let registration_tx = registration_tx.clone();
        let query_registration_tx = query_registration_tx.clone();

        let jwt_lock = jwt_lock.clone();
        tokio::spawn(async move {
            log::info!("Starting task scheduler...");
            init_task_scheduler(
                bot,
                registration_tx,
                job_rx,
                query_registration_tx,
                loaded_user_data,
                jwt_lock,
            )
            .await;
        });
    }

    // passing a receiver doesnt work for some reason, so sending query_registration_tx and resubscribing to get rx
    let command_handler_deps = dptree::deps![
        InMemStorage::<DialogueState>::new(),
        mensen,
        registration_tx,
        query_registration_tx,
        NO_DB_MSG,
        Backend::MensiMates,
        Some(jwt_lock)
    ];
    Dispatcher::builder(bot, schema())
        .dependencies(command_handler_deps)
        // .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

fn schema() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    use dptree::case;

    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(dptree::case![Command::Start].endpoint(start))
        .branch(dptree::case![Command::Heute].endpoint(day_cmd))
        .branch(dptree::case![Command::Morgen].endpoint(day_cmd))
        .branch(dptree::case![Command::Uebermorgen].endpoint(day_cmd))
        .branch(dptree::case![Command::Subscribe].endpoint(subscribe))
        .branch(dptree::case![Command::Unsubscribe].endpoint(unsubscribe))
        .branch(dptree::case![Command::Mensa].endpoint(change_mensa))
        .branch(dptree::case![Command::Uhrzeit].endpoint(start_time_dialogue));

    let message_handler = Update::filter_message()
        .branch(command_handler)
        .branch(case![DialogueState::AwaitTimeReply].endpoint(process_time_reply))
        .branch(dptree::endpoint(invalid_cmd));

    let callback_query_handler = Update::filter_callback_query().endpoint(callback_handler);

    dialogue::enter::<Update, InMemStorage<DialogueState>, DialogueState, _>()
        .branch(message_handler)
        .branch(callback_query_handler)
}

async fn start(bot: Bot, msg: Message, mensen: BTreeMap<&str, u8>) -> HandlerResult {
    let keyboard = make_mensa_keyboard(mensen, false);
    bot.send_message(msg.chat.id, "Mensa auswählen:")
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

async fn day_cmd(
    bot: Bot,
    msg: Message,
    cmd: Command,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    jwt_lock: Arc<RwLock<String>>,
    no_db_message: &str,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

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
        let text = build_meal_message_dispatcher(
            Backend::MensiMates,
            days_forward,
            registration.1,
            Some(jwt_lock),
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
        update_db_row(&task, MM_DB).unwrap();
        registration_tx.send(task).unwrap();
    } else {
        // every user has a registration (starting the chat always calls /start)
        // if this is none, it most likely means the DB was wiped)
        // forcing reregistration is better than crashing the bot, no data will be overwritten anyways
        // but copy pasting this everywhere is ugly
        bot.send_message(msg.chat.id, no_db_message).await?;
    }
    Ok(())
}

async fn subscribe(
    bot: Bot,
    msg: Message,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    no_db_message: &str,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

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
                        "Plan wird ab jetzt automatisch an Wochentagen 06:00 Uhr gesendet.\n\nÄndern mit /uhrzeit",
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
        bot.send_message(msg.chat.id, no_db_message).await?;
    }

    Ok(())
}

async fn unsubscribe(
    bot: Bot,
    msg: Message,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    no_db_message: &str,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

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
        bot.send_message(msg.chat.id, no_db_message).await?;
    }
    Ok(())
}

async fn change_mensa(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<&str, u8>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    no_db_message: &str,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

    registration_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    if query_registration_rx.recv().await.unwrap().is_some() {
        let keyboard = make_mensa_keyboard(mensen, true);
        bot.send_message(msg.chat.id, "Mensa auswählen:")
            .reply_markup(keyboard)
            .await?;
    } else {
        bot.send_message(msg.chat.id, no_db_message).await?;
    }
    Ok(())
}

async fn invalid_cmd(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Das ist kein Befehl.")
        .await?;
    Ok(())
}

async fn init_task_scheduler(
    bot: Bot,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    mut job_rx: broadcast::Receiver<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    mut loaded_user_data: HashMap<i64, RegistrationEntry>,
    jwt_lock: Arc<RwLock<String>>,
) {
    let registr_tx_loadjob = registration_tx.clone();
    let tasks_from_db = get_all_tasks_db(MM_DB);
    let sched = JobScheduler::new().await.unwrap();

    // if mensimates, create job to reload token every minute
    // reload now
    {
        let now = Instant::now();
        log::debug!("Updating JWT token");
        let jwt_lock = jwt_lock.clone();
        let mut wr = jwt_lock.write_owned().await;
        *wr = get_jwt_token().await;
        log::debug!("Got JWT: {:.2?}", now.elapsed());
    }
    {
        let jwt_lock = jwt_lock.clone();
        let jwt_job = Job::new_async("0 * * * * *", move |_uuid, mut _l| {
            let jwt_lock = jwt_lock.clone();
            Box::pin(async move {
                let now = Instant::now();
                log::debug!("Updating JWT token");
                let mut wr = jwt_lock.write_owned().await;
                *wr = get_jwt_token().await;
                log::debug!("Got JWT: {:.2?}", now.elapsed());
            })
        })
        .unwrap();
        sched.add(jwt_job).await.unwrap();
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
            Backend::MensiMates,
            Some(jwt_lock.clone()),
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
                init_db_record(&job_handler_task, MM_DB).unwrap();

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
                    Backend::MensiMates,
                    Some(jwt_lock.clone()),
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
                                Backend::MensiMates,
                                Some(jwt_lock.clone()),
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
                    update_db_row(&job_handler_task, MM_DB).unwrap();
                } else {
                    log::error!("Tried to update non-existent job");
                    bot.send_message(ChatId(job_handler_task.chat_id.unwrap()), NO_DB_MSG)
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
                task_db_kill_auto(job_handler_task.chat_id.unwrap(), MM_DB).unwrap();
            }

            JobType::QueryRegistration => {
                // check if uuid exists
                let uuid_opt = loaded_user_data.get(&job_handler_task.chat_id.unwrap());
                if uuid_opt.is_none() {
                    log::error!(target: "stuwe_telegram_rs::TS::Jobs", "chat_id {} sent a command, but is not in DB", &job_handler_task.chat_id.unwrap());
                }

                query_registration_tx.send(uuid_opt.copied()).unwrap();
            }

            JobType::InsertCallbackMessageId => {
                let mut regist = *loaded_user_data
                    .get(&job_handler_task.chat_id.unwrap())
                    .unwrap();

                regist.4 = job_handler_task.callback_id;

                loaded_user_data.insert(job_handler_task.chat_id.unwrap(), regist);
            }

            JobType::BroadcastUpdate => {}
        }
    }
}
