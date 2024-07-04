use chrono::Timelike;
use std::collections::BTreeMap;
use teloxide::{
    payloads::SendMessageSetters,
    requests::Requester,
    types::{ChatId, ParseMode},
    utils::markdown,
    Bot,
};
use tokio::sync::broadcast::Sender;
use tokio_cron_scheduler::{Job, JobScheduler};

use crate::{
    campusdual_fetcher::{
        compare_campusdual_grades, compare_campusdual_signup_options, get_campusdual_data,
        save_campusdual_grades, save_campusdual_signup_options,
    },
    constants::{BACKEND, CD_DATA, NO_DB_MSG},
    data_backend::stuwe_parser::update_cache,
    data_types::{Backend, BroadcastUpdateTask, JobHandlerTask, JobType, RegistrationEntry},
    db_operations::{
        get_all_user_registrations_db, init_db_record, task_db_kill_auto, update_db_row,
    },
    shared_main::{build_meal_message_dispatcher, load_job},
};

pub async fn handle_add_registration_task(
    bot: &Bot,
    job_handler_task: JobHandlerTask,
    sched: &JobScheduler,
    loaded_user_data: &mut BTreeMap<i64, RegistrationEntry>,
    jobhandler_task_tx: Sender<JobHandlerTask>,
) {
    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Register: {} for Mensa {}", &job_handler_task.chat_id.unwrap(), &job_handler_task.mensa_id.unwrap());
    // creates a new row, or replaces every col with new values
    init_db_record(&job_handler_task).unwrap();
    if let Some(previous_registration) = loaded_user_data.get(&job_handler_task.chat_id.unwrap()) {
        if let Some(uuid) = previous_registration.job_uuid {
            sched.context.job_delete_tx.send(uuid).unwrap();
        };
    }

    // get uuid (here guaranteed to be Some() since default is registration with job)
    let new_uuid = load_job(
        bot.clone(),
        sched,
        job_handler_task.clone(),
        jobhandler_task_tx,
    )
    .await;

    // insert new job uuid
    loaded_user_data.insert(
        job_handler_task.chat_id.unwrap(),
        RegistrationEntry {
            job_uuid: new_uuid,
            mensa_id: job_handler_task.mensa_id.unwrap(),
            hour: job_handler_task.hour,
            minute: job_handler_task.minute,
        },
    );
}

pub async fn handle_update_registration_task(
    bot: &Bot,
    job_handler_task: JobHandlerTask,
    sched: &JobScheduler,
    loaded_user_data: &mut BTreeMap<i64, RegistrationEntry>,
    jobhandler_task_tx: Sender<JobHandlerTask>,
) {
    if let Some(mensa) = job_handler_task.mensa_id {
        log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{} 📌 to {}", job_handler_task.chat_id.unwrap(), mensa);
    }
    if job_handler_task.hour.is_some() {
        log::info!(target: "stuwe_telegram_rs::TS::Jobs", "{} changed 🕘: {:02}:{:02}", job_handler_task.chat_id.unwrap(), job_handler_task.hour.unwrap(), job_handler_task.minute.unwrap());
    }

    if let Some(previous_registration) = loaded_user_data.get(&job_handler_task.chat_id.unwrap()) {
        let mensa_id = job_handler_task
            .mensa_id
            .unwrap_or(previous_registration.mensa_id);
        let hour = job_handler_task.hour.or(previous_registration.hour);
        let minute = job_handler_task.minute.or(previous_registration.minute);

        let new_job_task = JobHandlerTask {
            job_type: JobType::UpdateRegistration,
            chat_id: job_handler_task.chat_id,
            mensa_id: Some(mensa_id),
            hour,
            minute,
        };

        let new_uuid =
            // new time was passed -> unload old job, load new
            if job_handler_task.hour.is_some() || job_handler_task.minute.is_some() || job_handler_task.mensa_id.is_some() {
                if let Some(uuid) = previous_registration.job_uuid {
                    // unload old job if exists
                    sched.context.job_delete_tx.send(uuid).unwrap();
                }
                // load new job, return uuid
                load_job(
                    bot.clone(),
                    sched,
                    new_job_task.clone(),
                    jobhandler_task_tx.clone(),
                ).await
            } else {
                // no new time was set -> return old job uuid
                previous_registration.job_uuid
            };

        loaded_user_data.insert(
            job_handler_task.chat_id.unwrap(),
            RegistrationEntry {
                job_uuid: new_uuid,
                mensa_id,
                hour,
                minute,
            },
        );

        // update any values that are to be changed, aka are Some()
        update_db_row(&job_handler_task).unwrap();
    } else {
        log::error!("Tried to update non-existent job");
        bot.send_message(ChatId(job_handler_task.chat_id.unwrap()), NO_DB_MSG)
            .await
            .unwrap();
    }
}

pub async fn handle_delete_registration_task(
    job_handler_task: JobHandlerTask,
    sched: &JobScheduler,
    loaded_user_data: &mut BTreeMap<i64, RegistrationEntry>,
) {
    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Unregister: {}", &job_handler_task.chat_id.unwrap());

    // unregister is only invoked if existence of job is guaranteed
    let previous_registration = loaded_user_data
        .get(&job_handler_task.chat_id.unwrap())
        .unwrap();

    // unload old job
    sched
        .context
        .job_delete_tx
        .send(previous_registration.job_uuid.unwrap())
        .unwrap();

    // kill uuid from this thing
    loaded_user_data.insert(
        job_handler_task.chat_id.unwrap(),
        RegistrationEntry {
            job_uuid: None,
            mensa_id: previous_registration.mensa_id,
            hour: None,
            minute: None,
            // last_markup_id,
        },
    );

    // delete from db
    task_db_kill_auto(job_handler_task.chat_id.unwrap()).unwrap();
}

pub async fn handle_query_registration_task(
    job_handler_task: JobHandlerTask,
    loaded_user_data: &mut BTreeMap<i64, RegistrationEntry>,
    user_registration_data_tx: Sender<Option<RegistrationEntry>>,
) {
    // check if uuid exists
    let opt_registration_entry = loaded_user_data.get(&job_handler_task.chat_id.unwrap());
    if opt_registration_entry.is_none() {
        log::warn!(target: "stuwe_telegram_rs::TS::QueryReg", "chat_id {} has no registration", &job_handler_task.chat_id.unwrap());
    }

    user_registration_data_tx
        .send(opt_registration_entry.copied())
        .unwrap();
}

pub async fn handle_broadcast_update_task(
    bot: &Bot,
    job_handler_task: JobHandlerTask,
    loaded_user_data: &mut BTreeMap<i64, RegistrationEntry>,
) {
    log::info!(target: "stuwe_telegram_rs::TS::Jobs", "TodayMeals changed @Mensa {}", &job_handler_task.mensa_id.unwrap());
    for (chat_id, registration_data) in loaded_user_data {
        let mensa_id = registration_data.mensa_id;

        let now = chrono::Local::now();

        if let (Some(job_hour), Some(job_minute)) =
            (registration_data.hour, registration_data.minute)
        {
            // send update to all subscribers of this mensa id
            if mensa_id == job_handler_task.mensa_id.unwrap()
                        // only send updates after job message has been sent: job hour has to be earlier OR same hour, earlier minute
                        && (job_hour < now.hour() || job_hour == now.hour() && job_minute <= now.minute())
            {
                log::info!(target: "stuwe_telegram_rs::TS::Jobs", "Sent update to {}", chat_id);

                let text = format!(
                    "{}\n{}",
                    &markdown::bold(&markdown::underline("Planänderung:")),
                    build_meal_message_dispatcher(0, mensa_id).await
                );

                bot.send_message(ChatId(*chat_id), text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await
                    .unwrap();
            }
        }
    }
}

pub async fn start_mensacache_and_campusdual_job(
    bot: Bot,
    sched: &JobScheduler,
    job_handler_tx: Sender<JobHandlerTask>,
) {
    let cache_and_broadcast_job = Job::new_async("0 0/5 * * * *", move |_uuid, mut _l| {
        let bot = bot.clone();
        let job_handler_tx = job_handler_tx.clone();

        Box::pin(async move {
            if *BACKEND.get().unwrap() == Backend::StuWe {
                log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating Mensae");

                match update_cache().await {
                    Ok(today_changed_mensen) => {
                        for mensa_id in today_changed_mensen {
                            job_handler_tx
                                .send(BroadcastUpdateTask { mensa_id }.into())
                                .unwrap();
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to update cache: {}", e);
                    }
                }
            }
            log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating CampusDual");
            check_notify_campusdual_grades_signups(bot).await;
        })
    })
    .unwrap();
    sched.add(cache_and_broadcast_job).await.unwrap();
}

async fn check_notify_campusdual_grades_signups(bot: Bot) {
    if let Some(cd_data) = CD_DATA.get() {
        match get_campusdual_data(cd_data.username.clone(), cd_data.password.clone()).await {
            Ok((grades, signup_options)) => {
                if let Some(new_grades) = compare_campusdual_grades(&grades).await {
                    log::info!("Got new grades! Sending to {}", cd_data.chat_id);

                    let mut msg = String::from("Neue Note:");
                    for grade in new_grades {
                        msg.push_str(&format!("\n{}: {}", grade.name, grade.grade));
                    }
                    match bot.send_message(ChatId(cd_data.chat_id), msg).await {
                        Ok(_) => save_campusdual_grades(&grades).await,
                        Err(e) => {
                            log::error!("Failed to send CD grades: {}", e);
                        }
                    }
                }
                if let Some(new_signup_options) =
                    compare_campusdual_signup_options(&signup_options).await
                {
                    log::info!("Got new signup options! Sending to {}", cd_data.chat_id);

                    let mut msg = String::from("Neue Anmeldemöglichkeit:");
                    for signup_option in new_signup_options {
                        msg.push_str(&format!(
                            "\n{} ({}) — {}",
                            signup_option.status, signup_option.verfahren, signup_option.name
                        ));
                    }
                    match bot.send_message(ChatId(cd_data.chat_id), msg).await {
                        Ok(_) => save_campusdual_signup_options(&signup_options).await,
                        Err(e) => {
                            log::error!("Failed to send CD signup options: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to get CD grades: {}", e);
            }
        }
    }
}

pub async fn load_jobs_from_db(
    bot: &Bot,
    sched: &JobScheduler,
    loaded_user_data: &mut BTreeMap<i64, RegistrationEntry>,
    jobhandler_task_tx: Sender<JobHandlerTask>,
) {
    let tasks_from_db = get_all_user_registrations_db().unwrap();

    for task in tasks_from_db {
        let bot = bot.clone();
        let jobhandler_task_tx = jobhandler_task_tx.clone();

        let uuid = load_job(bot, sched, task.clone(), jobhandler_task_tx).await;
        loaded_user_data.insert(
            task.chat_id.unwrap(),
            RegistrationEntry {
                job_uuid: uuid,
                mensa_id: task.mensa_id.unwrap(),
                hour: task.hour,
                minute: task.minute,
            },
        );
    }
}
