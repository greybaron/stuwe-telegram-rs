use std::{collections::BTreeMap, env, error::Error, sync::Arc, time::Instant};

use chrono::Timelike;
use regex_lite::Regex;
use static_init::dynamic;
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
    data_backend::{mm_parser::mm_b_meal_msg, stuwe_parser::stuwe_b_meal_msg},
    data_types::{Backend, Command, NO_DB_MSG},
};
use crate::{
    data_types::{
        DialogueState, DialogueType, HandlerResult, JobHandlerTask, JobType, RegistrationEntry,
        TimeParseError,
    },
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

pub async fn start_time_dialogue(
    bot: Bot,
    msg: Message,
    dialogue: DialogueType,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    no_db_message: &str,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

    registration_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    let qr_reg = query_registration_rx.recv().await.unwrap();
    // if query_registration_rx.recv().await.unwrap().is_some() {
    if let Some(regist) = qr_reg {
        log::error!(
            "REG: {}:{} &{}",
            regist.2.unwrap_or_default(),
            regist.3.unwrap_or_default(),
            regist.1
        );
        let argument = msg.text().unwrap_or_default().split(' ').nth(1);
        let parsed_opt = if argument.is_some() {
            Some(parse_time(argument.unwrap()))
        } else {
            None
        };

        if parsed_opt.is_some() && parsed_opt.as_ref().unwrap().is_ok() {
            let (hour, minute) = parsed_opt.unwrap().unwrap();
            bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/unsubscribe zum Deaktivieren", hour, minute)).await?;
            dialogue.exit().await.unwrap();

            let registration_job = JobHandlerTask {
                job_type: JobType::UpdateRegistration,
                chat_id: Some(msg.chat.id.0),
                mensa_id: None,
                hour: Some(hour),
                minute: Some(minute),
                callback_id: None,
            };
            registration_tx.send(registration_job).unwrap();
        } else {
            bot.send_message(msg.chat.id, "Bitte mit Uhrzeit antworten:")
                .await
                .unwrap();
            dialogue
                .update(DialogueState::AwaitTimeReply)
                .await
                .unwrap();
        };
    } else {
        bot.send_message(msg.chat.id, no_db_message).await.unwrap();
        dialogue.exit().await.unwrap();
    }
    Ok(())
}

pub async fn process_time_reply(
    bot: Bot,
    msg: Message,
    dialogue: DialogueType,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    no_db_message: &str,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

    registration_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    if query_registration_rx.recv().await.unwrap().is_some() {
        if let Some(txt) = msg.text() {
            match parse_time(txt) {
                Ok((hour, minute)) => {
                    bot.send_message(msg.chat.id, format!("Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr gesendet.\n\n/unsubscribe zum Deaktivieren", hour, minute)).await?;
                    dialogue.exit().await.unwrap();

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
                Err(TimeParseError::InvalidTimePassed) => {
                    bot.send_message(
                        msg.chat.id,
                        "Eingegebene Zeit ist ungültig.\nBitte mit Uhrzeit antworten:",
                    )
                    .await?;
                }
            }
        } else {
            bot.send_message(
                msg.chat.id,
                "Das ist kein Text.\nBitte mit Uhrzeit antworten:",
            )
            .await?;
        };
    } else {
        bot.send_message(msg.chat.id, no_db_message).await?;
        dialogue.exit().await.unwrap();
    }

    Ok(())
}

fn parse_time(txt: &str) -> Result<(u32, u32), TimeParseError> {
    #[dynamic]
    static RE: Regex = Regex::new("^([01]?[0-9]|2[0-3]):([0-5][0-9])").unwrap();
    if !RE.is_match(txt) {
        Err(TimeParseError::InvalidTimePassed)
    } else {
        let caps = RE.captures(txt).unwrap();

        let hour = caps.get(1).unwrap().as_str().parse::<u32>().unwrap();
        let minute = caps.get(2).unwrap().as_str().parse::<u32>().unwrap();

        Ok((hour, minute))
    }
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
    mensen: BTreeMap<&str, u8>,
    only_mensa_upd: bool,
) -> InlineKeyboardMarkup {
    let mut keyboard = Vec::new();

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
        Backend::MensiMates => mm_b_meal_msg(days_forward, mensa_location, jwt_lock.unwrap()).await,
        Backend::StuWe => stuwe_b_meal_msg(days_forward, mensa_location).await,
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
    mensen: BTreeMap<&str, u8>,
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
                        *mensen.get(arg).unwrap(),
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
                        mensa_id: Some(*mensen.get(arg).unwrap()),
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
                        *mensen.get(arg).unwrap(),
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
                        job_type: JobType::Register,
                        chat_id: Some(chat.id.0),
                        mensa_id: Some(*mensen.get(arg).unwrap()),
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
