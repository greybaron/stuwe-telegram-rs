use std::{collections::BTreeMap, env, error::Error, sync::Arc, time::Instant};

use chrono::Timelike;
use teloxide::{
    prelude::*,
    utils::{command::BotCommands, markdown},
};
use teloxide_core::{
    types::{InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId, ParseMode},
    Bot,
};
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use crate::{
    data_backend::{mm_parser::mm_build_meal_msg, stuwe_parser::stuwe_build_meal_msg},
    data_types::{Backend, Command, NO_DB_MSG},
};
use crate::{
    data_types::{JobHandlerTask, JobType, RegistrationEntry},
    db_operations::update_db_row,
};

pub fn logger_init(module_path: &str) {
    pretty_env_logger::formatted_timed_builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module(
            module_path,
            if env::var(pretty_env_logger::env_logger::DEFAULT_FILTER_ENV).unwrap_or_default()
                == "debug"
            {
                log::LevelFilter::Debug
            } else {
                log::LevelFilter::Info
            },
        )
        .init();
}

pub fn make_query_data(chat_id: i64) -> JobHandlerTask {
    JobHandlerTask {
        job_type: JobType::QueryRegistration,
        chat_id: Some(chat_id),
        mensa_id: None,
        hour: None,
        minute: None,
        callback_id: None,
    }
}

pub fn make_mensa_keyboard(
    mensen: BTreeMap<u8, String>,
    only_mensa_upd: bool,
) -> InlineKeyboardMarkup {
    let mut keyboard = Vec::new();

    for mensa in mensen {
        keyboard.push([InlineKeyboardButton::callback(
            &mensa.1,
            format!(
                "m_{}:{}",
                if only_mensa_upd { "upd" } else { "regist" },
                &mensa.1
            ),
        )]);
        // }
    }
    InlineKeyboardMarkup::new(keyboard)
}

pub async fn make_days_keyboard(
    bot: &Bot,
    chat_id: i64,
    previous_markup_id: Option<i32>,
) -> InlineKeyboardMarkup {
    if let Some(id) = previous_markup_id {
        _ = bot
            .edit_message_reply_markup(ChatId(chat_id), MessageId(id))
            .await;
    };

    let keyboard = vec![vec![
        InlineKeyboardButton::callback("Heute", "day:0"),
        InlineKeyboardButton::callback("Morgen", "day:1"),
        InlineKeyboardButton::callback("Überm.", "day:2"),
    ]];
    InlineKeyboardMarkup::new(keyboard)
}

pub async fn build_meal_message_dispatcher(
    backend: Backend,
    days_forward: i64,
    mensa_location: u8,
    jwt_lock: Option<Arc<RwLock<String>>>,
) -> String {
    match backend {
        Backend::MensiMates => {
            mm_build_meal_msg(days_forward, mensa_location, jwt_lock.unwrap()).await
        }
        Backend::StuWe => stuwe_build_meal_msg(days_forward, mensa_location).await,
    }
}

pub async fn load_job(
    bot: Bot,
    sched: &JobScheduler,
    task: JobHandlerTask,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_rx: broadcast::Receiver<Option<RegistrationEntry>>,
    // jwt_lock is only used for mensimates
    backend: Backend,
    jwt_lock: Option<Arc<RwLock<String>>>,
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
            let jwt_lock = jwt_lock.clone();

            let bot = bot.clone();
            let registration_tx = registration_tx.clone();
            let mut query_registration_rx = query_registration_rx.resubscribe();

            Box::pin(async move {
                registration_tx
                    .send(make_query_data(task.chat_id.unwrap()))
                    .unwrap();
                let previous_markup_id = query_registration_rx.recv().await.unwrap().unwrap().4;

                let keyboard =
                    make_days_keyboard(&bot, task.chat_id.unwrap(), previous_markup_id).await;
                let markup_id = bot
                    .send_message(
                        ChatId(task.chat_id.unwrap()),
                        build_meal_message_dispatcher(backend, 0, task.mensa_id.unwrap(), jwt_lock)
                            .await,
                    )
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .await
                    .unwrap()
                    .id
                    .0;

                // send message callback id to store
                let task = JobHandlerTask {
                    job_type: JobType::InsertMarkupMessageID,
                    chat_id: Some(task.chat_id.unwrap()),
                    mensa_id: None,
                    hour: None,
                    minute: None,
                    callback_id: Some(markup_id),
                };

                update_db_row(&task, backend).unwrap();
                registration_tx.send(task).unwrap();
            })
        },
    )
    .unwrap();

    let uuid = job.guid();
    sched.add(job).await.unwrap();

    Some(uuid)
}

pub async fn callback_handler(
    backend: Backend,
    jwt_lock: Option<Arc<RwLock<String>>>,
    bot: Bot,
    q: CallbackQuery,
    mensen: BTreeMap<u8, String>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
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

                    let text = build_meal_message_dispatcher(
                        backend,
                        0,
                        *mensen.iter().find(|(_, v)| v.as_str() == arg).unwrap().0,
                        jwt_lock,
                    )
                    .await;

                    registration_tx.send(make_query_data(chat.id.0)).unwrap();
                    let previous_markup_id = query_registration_rx.recv().await.unwrap().unwrap().4;

                    let keyboard = make_days_keyboard(&bot, chat.id.0, previous_markup_id).await;
                    let markup_id = bot
                        .send_message(chat.id, text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?
                        .id
                        .0;

                    let task = JobHandlerTask {
                        job_type: JobType::UpdateRegistration,
                        chat_id: Some(chat.id.0),
                        mensa_id: Some(*mensen.iter().find(|(_, v)| v.as_str() == arg).unwrap().0),
                        hour: None,
                        minute: None,
                        callback_id: Some(markup_id),
                    };

                    update_db_row(&task, backend).unwrap();
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

                    bot.send_message(chat.id, format!("Plan der {} wird ab jetzt automatisch an Wochentagen *06:00 Uhr* gesendet\\.\n\nÄndern mit\n/mensa oder /uhrzeit", markdown::bold(arg)))
                    .parse_mode(ParseMode::MarkdownV2).await?;

                    let text = build_meal_message_dispatcher(
                        backend,
                        0,
                        *mensen.iter().find(|(_, v)| v.as_str() == arg).unwrap().0,
                        jwt_lock,
                    )
                    .await;

                    registration_tx.send(make_query_data(chat.id.0)).unwrap();
                    let previous_registration = query_registration_rx.recv().await.unwrap();
                    let previous_markup_id =
                        if let Some(previous_registration) = previous_registration {
                            previous_registration.4
                        } else {
                            None
                        };

                    let keyboard = make_days_keyboard(&bot, chat.id.0, previous_markup_id).await;
                    let markup_id = bot
                        .send_message(chat.id, text)
                        .parse_mode(ParseMode::MarkdownV2)
                        .reply_markup(keyboard)
                        .await?
                        .id
                        .0;

                    let task = JobHandlerTask {
                        job_type: JobType::Register,
                        chat_id: Some(chat.id.0),
                        mensa_id: Some(*mensen.iter().find(|(_, v)| v.as_str() == arg).unwrap().0),
                        hour: Some(6),
                        minute: Some(0),
                        callback_id: Some(markup_id),
                    };

                    update_db_row(&task, backend).unwrap();
                    registration_tx.send(task).unwrap();
                }
                "day" => {
                    registration_tx.send(make_query_data(chat.id.0)).unwrap();
                    let prev_reg_opt = query_registration_rx.recv().await.unwrap();
                    if let Some(prev_registration) = prev_reg_opt {
                        // start building message
                        let now = Instant::now();

                        let days_forward = arg.parse::<i64>().unwrap();
                        let day_str = ["Heute", "Morgen", "Übermorgen"]
                            [usize::try_from(days_forward).unwrap()];

                        let text = build_meal_message_dispatcher(
                            backend,
                            days_forward,
                            prev_registration.1,
                            jwt_lock,
                        )
                        .await;
                        log::debug!("Build {} msg: {:.2?}", day_str, now.elapsed());
                        let now = Instant::now();

                        registration_tx.send(make_query_data(chat.id.0)).unwrap();
                        let previous_markup_id =
                            query_registration_rx.recv().await.unwrap().unwrap().4;

                        let keyboard =
                            make_days_keyboard(&bot, chat.id.0, previous_markup_id).await;
                        let markup_id = bot
                            .send_message(chat.id, text)
                            .parse_mode(ParseMode::MarkdownV2)
                            .reply_markup(keyboard)
                            .await?
                            .id
                            .0;
                        log::debug!("Send {} msg: {:.2?}", day_str, now.elapsed());

                        let task = JobHandlerTask {
                            job_type: JobType::InsertMarkupMessageID,
                            chat_id: Some(chat.id.0),
                            mensa_id: None,
                            hour: None,
                            minute: None,
                            callback_id: Some(markup_id),
                        };

                        update_db_row(&task, backend).unwrap();
                        registration_tx.send(task).unwrap();

                        // also try to remove answered query anyway, as a fallback (the more the merrier)
                        _ = bot.edit_message_reply_markup(chat.id, id).await;
                    } else {
                        bot.send_message(chat.id, NO_DB_MSG).await?;
                    }
                }
                _ => panic!("Unknown callback query command: {}", cmd),
            }
        }
    }

    Ok(())
}
