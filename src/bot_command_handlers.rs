#![allow(unused_imports)]
use crate::data_backend::mm_parser::get_jwt_token;
use crate::data_types::{
    Backend, Command, DialogueState, DialogueType, HandlerResult, JobHandlerTask,
    JobHandlerTaskType, JobType, QueryRegistrationType, RegistrationEntry, TimeParseError, MENSEN,
    MM_DB, NO_DB_MSG,
};
use crate::db_operations::{get_all_tasks_db, init_db_record, task_db_kill_auto, update_db_row};
use crate::shared_main::{
    build_meal_message_dispatcher, callback_handler, load_job, logger_init, make_days_keyboard,
    make_mensa_keyboard, make_query_data,
};

use log::log_enabled;
use regex_lite::Regex;
use static_init::dynamic;
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

pub async fn start(bot: Bot, msg: Message, mensen: BTreeMap<&str, u8>) -> HandlerResult {
    let keyboard = make_mensa_keyboard(mensen, false);
    bot.send_message(msg.chat.id, "Mensa auswählen:")
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

pub async fn unsubscribe(
    bot: Bot,
    msg: Message,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
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
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }
    Ok(())
}

pub async fn day_cmd(
    bot: Bot,
    msg: Message,
    cmd: Command,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    backend: Backend,
    jwt_lock: Option<Arc<RwLock<String>>>,
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
        let text =
            build_meal_message_dispatcher(backend, days_forward, registration.1, jwt_lock).await;
        log::debug!("Build {:?} msg: {:.2?}", cmd, now.elapsed());
        let now = Instant::now();

        // registration_tx
        //     .send(make_query_data(msg.chat.id.0))
        //     .unwrap();
        // let previous_markup_id = query_registration_rx.recv().await.unwrap().unwrap().4;
        let previous_markup_id = registration.4;

        let keyboard = make_days_keyboard(&bot, msg.chat.id.0, previous_markup_id).await;
        let markup_id = bot
            .send_message(msg.chat.id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .reply_markup(keyboard)
            .await?
            .id
            .0;

        log::debug!("Send {:?} msg: {:.2?}", cmd, now.elapsed());

        // send message callback id to store
        let task = JobHandlerTask {
            job_type: JobType::InsertMarkupMessageID,
            chat_id: Some(msg.chat.id.0),
            mensa_id: None,
            hour: None,
            minute: None,
            callback_id: Some(markup_id),
        };

        // save to db
        update_db_row(&task, backend).unwrap();
        // update live state which sounds way cooler than btree
        registration_tx.send(task).unwrap();
    } else {
        // every user has a registration (starting the chat always calls /start)
        // if this is none, it most likely means the DB was wiped)
        // forcing reregistration is better than crashing the bot, no data will be overwritten anyways
        // but copy pasting this everywhere is ugly
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }
    Ok(())
}

pub async fn subscribe(
    bot: Bot,
    msg: Message,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
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
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }

    Ok(())
}

pub async fn change_mensa(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<&str, u8>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
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
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }
    Ok(())
}

pub async fn start_time_dialogue(
    bot: Bot,
    msg: Message,
    dialogue: DialogueType,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

    registration_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    let qr_reg = query_registration_rx.recv().await.unwrap();
    // if query_registration_rx.recv().await.unwrap().is_some() {
    if let Some(regist) = qr_reg {
        log::error!(
            "REG: {:02}:{:02} 📌{}",
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
        bot.send_message(msg.chat.id, NO_DB_MSG).await.unwrap();
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
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
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
