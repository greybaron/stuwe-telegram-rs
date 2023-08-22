use stuwe_telegram_rs::bot_command_handlers::{
    change_mensa, day_cmd, process_time_reply, start, start_time_dialogue, subscribe, unsubscribe,
};
use stuwe_telegram_rs::data_backend::stuwe_parser::update_cache;
use stuwe_telegram_rs::data_types::{
    Backend, Command, DialogueState, HandlerResult, JobHandlerTask, JobHandlerTaskType, JobType,
    QueryRegistrationType, RegistrationEntry, MENSEN, NO_DB_MSG, STUWE_DB,
};
use stuwe_telegram_rs::db_operations::{
    get_all_tasks_db, init_db_record, task_db_kill_auto, update_db_row,
};
use stuwe_telegram_rs::shared_main::{
    build_meal_message_dispatcher, callback_handler, load_job, logger_init, make_days_keyboard,
};

use chrono::Timelike;
use log::log_enabled;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use teloxide::{
    dispatching::{
        dialogue::{self, InMemStorage},
        UpdateHandler,
    },
    prelude::*,
    types::ParseMode,
    utils::markdown,
};
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler};

#[tokio::main]
async fn main() {
    logger_init(module_path!());

    log::info!("Starting command bot...");

    let bot = Bot::from_env();
    let mensen = BTreeMap::from(MENSEN);

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
        let mensen = mensen.clone();
        tokio::spawn(async move {
            log::info!("Starting task scheduler...");
            init_task_scheduler(
                bot,
                registration_tx,
                job_rx,
                query_registration_tx,
                loaded_user_data,
                mensen.values().copied().collect(),
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
        Backend::StuWe,
        None::<Arc<RwLock<String>>>
    ];
    Dispatcher::builder(bot, schema())
        .dependencies(command_handler_deps)
        .enable_ctrlc_handler()
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
    mensen_ids: Vec<u8>,
) {
    let backend = Backend::StuWe;
    let registr_tx_loadjob = registration_tx.clone();
    let tasks_from_db = get_all_tasks_db(STUWE_DB);
    let sched = JobScheduler::new().await.unwrap();

    // always update cache on startup
    // start caching every 5 minutes and cache once at startup
    log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache...");
    update_cache(&mensen_ids).await;
    log::info!(target: "stuwe_telegram_rs::TaskSched", "Cache updated!");

    // run cache update every 5 minutes
    let registration_tx_istg = registration_tx.clone();
    let cache_and_broadcast_job = Job::new_async("0 0/5 * * * *", move |_uuid, mut _l| {
        log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache");

        let registration_tx = registration_tx_istg.clone();

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

    for task in tasks_from_db {
        let bot = bot.clone();
        let registration_tx = registr_tx_loadjob.clone();

        let uuid = load_job(
            bot,
            &sched,
            task.clone(),
            registration_tx,
            query_registration_tx.subscribe(),
            Backend::StuWe,
            None,
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
                init_db_record(&job_handler_task, STUWE_DB).unwrap();

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
                    Backend::StuWe,
                    None,
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
            // KEEP THIS ONE
            JobType::UpdateRegistration => {
                if let Some(mensa) = job_handler_task.mensa_id {
                    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{} changed M📌 to {}", job_handler_task.chat_id.unwrap(), mensa);
                }
                if job_handler_task.hour.is_some() {
                    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{} changed 🕘: {:2}:{:2}", job_handler_task.chat_id.unwrap(), job_handler_task.hour.unwrap(), job_handler_task.minute.unwrap());
                }

                if let Some(previous_registration) =
                    loaded_user_data.get(&job_handler_task.chat_id.unwrap())
                {
                    let mensa_id = job_handler_task.mensa_id.unwrap_or(previous_registration.1);
                    let hour = job_handler_task.hour.or(previous_registration.2);
                    let minute = job_handler_task.minute.or(previous_registration.3);

                    let new_job_task = JobHandlerTask {
                        job_type: JobType::UpdateRegistration,
                        chat_id: job_handler_task.chat_id,
                        mensa_id: Some(mensa_id),
                        hour,
                        minute,
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
                                Backend::StuWe,
                                None
                            ).await
                        } else {
                            // no new time was set -> return old job uuid
                            previous_registration.0
                        };

                    loaded_user_data.insert(
                        job_handler_task.chat_id.unwrap(),
                        (new_uuid, mensa_id, hour, minute, previous_registration.4),
                    );

                    // update any values that are to be changed, aka are Some()
                    update_db_row(&job_handler_task, backend).unwrap();
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
                task_db_kill_auto(job_handler_task.chat_id.unwrap(), STUWE_DB).unwrap();
            }

            JobType::QueryRegistration => {
                // check if uuid exists
                let uuid_opt = loaded_user_data.get(&job_handler_task.chat_id.unwrap());
                if uuid_opt.is_none() {
                    log::warn!(target: "stuwe_telegram_rs::TS::QueryReg", "chat_id {} has no registration", &job_handler_task.chat_id.unwrap());
                }

                query_registration_tx.send(uuid_opt.copied()).unwrap();
            }

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

                            let text = format!(
                                "{}\n{}",
                                &markdown::bold(&markdown::underline("Planänderung:")),
                                build_meal_message_dispatcher(Backend::StuWe, 0, mensa_id, None)
                                    .await
                            );

                            let previous_markup_id = registration_data.4;
                            let keyboard =
                                make_days_keyboard(&bot, *chat_id, previous_markup_id).await;
                            let markup_id = bot
                                .send_message(ChatId(*chat_id), text)
                                .parse_mode(ParseMode::MarkdownV2)
                                .reply_markup(keyboard)
                                .await
                                .unwrap()
                                .id
                                .0;

                            let task = JobHandlerTask {
                                job_type: JobType::InsertMarkupMessageID,
                                chat_id: Some(*chat_id),
                                mensa_id: None,
                                hour: None,
                                minute: None,
                                callback_id: Some(markup_id),
                            };

                            update_db_row(&task, backend).unwrap();
                            registration_tx.send(task).unwrap();
                        }
                    }
                }
            }

            JobType::InsertMarkupMessageID => {
                let mut regist = *loaded_user_data
                    .get(&job_handler_task.chat_id.unwrap())
                    .unwrap();

                regist.4 = job_handler_task.callback_id;

                loaded_user_data.insert(job_handler_task.chat_id.unwrap(), regist);
            }
        }
    }
}
