use crate::bot_command_helpers::{
    mensa_disp_or_upd, parse_time_send_status_msgs, send_bloat_image,
};
use crate::constants::NO_DB_MSG;
use crate::data_types::{
    Command, DialogueState, DialogueType, HandlerResult, JobHandlerTask, MensaKeyboardAction,
    RegistrationEntry, UnregisterTask, UpdateRegistrationTask,
};

use crate::shared_main::{build_meal_message_dispatcher, make_mensa_keyboard, make_query_data};
use rand::Rng;
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use teloxide::{prelude::*, types::ParseMode};
use tokio::sync::{broadcast, RwLock};

pub async fn start(bot: Bot, msg: Message, mensen: BTreeMap<u8, String>) -> HandlerResult {
    let keyboard = make_mensa_keyboard(mensen, MensaKeyboardAction::Register);
    bot.send_message(msg.chat.id, "Mensa auswählen:")
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

pub async fn invalid_cmd(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Das ist kein Befehl.")
        .await?;
    Ok(())
}

pub async fn day_cmd(
    bot: Bot,
    msg: Message,
    cmd: Command,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
    jwt_lock: Option<Arc<RwLock<String>>>,
) -> HandlerResult {
    let mut user_registration_data_rx = user_registration_data_tx.subscribe();

    let days_forward = match cmd {
        Command::Heute => 0,
        Command::Morgen => 1,
        Command::Uebermorgen => 2,
        _ => unreachable!(),
    };

    let now = Instant::now();
    jobhandler_task_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();

    if let Some(registration) = user_registration_data_rx.recv().await.unwrap() {
        let text =
            build_meal_message_dispatcher(days_forward, registration.mensa_id, jwt_lock).await;
        log::debug!("Build {:?} msg: {:.2?}", cmd, now.elapsed());
        let now = Instant::now();

        bot.send_message(msg.chat.id, text)
            .parse_mode(ParseMode::MarkdownV2)
            .await?;

        log::debug!("Send {:?} msg: {:.2?}", cmd, now.elapsed());
    } else {
        // every user has a registration (starting the chat always calls /start)
        // if this is none, it most likely means the DB was wiped)
        // forcing reregistration is better than crashing the bot, no data will be overwritten anyways
        // but copy pasting this everywhere is ugly
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }
    Ok(())
}

pub async fn show_different_mensa(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<u8, String>,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    mensa_disp_or_upd(
        bot,
        msg,
        mensen,
        jobhandler_task_tx,
        user_registration_data_tx,
        MensaKeyboardAction::DisplayOnce,
    )
    .await
}

pub async fn subscribe(
    bot: Bot,
    msg: Message,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    let mut user_registration_data_rx = user_registration_data_tx.subscribe();

    jobhandler_task_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    if let Some(registration) = user_registration_data_rx.recv().await.unwrap() {
        jobhandler_task_tx
            .send(make_query_data(msg.chat.id.0))
            .unwrap();

        if registration.job_uuid.is_some() {
            if rand::thread_rng().gen_range(0..10) == 0 {
                send_bloat_image(&bot, msg.chat.id).await;
            }

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

            let registration_job = UpdateRegistrationTask {
                chat_id: msg.chat.id.0,
                mensa_id: None,
                hour: Some(6),
                minute: Some(0),
            }
            .into();

            jobhandler_task_tx.send(registration_job).unwrap();
        }
    } else {
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }

    Ok(())
}

pub async fn unsubscribe(
    bot: Bot,
    msg: Message,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    let mut user_registration_data_rx = user_registration_data_tx.subscribe();

    jobhandler_task_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    if let Some(registration) = user_registration_data_rx.recv().await.unwrap() {
        if registration.job_uuid.is_none() {
            bot.send_message(
                msg.chat.id,
                "Automatische Nachrichten waren bereits deaktiviert.",
            )
            .await?;
        } else {
            bot.send_message(msg.chat.id, "Plan wird nicht mehr automatisch gesendet.")
                .await?;

            jobhandler_task_tx
                .send(
                    UnregisterTask {
                        chat_id: msg.chat.id.0,
                    }
                    .into(),
                )
                .unwrap();
        }
    } else {
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }
    Ok(())
}

pub async fn change_mensa(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<u8, String>,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    mensa_disp_or_upd(
        bot,
        msg,
        mensen,
        jobhandler_task_tx,
        user_registration_data_tx,
        MensaKeyboardAction::Update,
    )
    .await
}

pub async fn start_time_dialogue(
    bot: Bot,
    msg: Message,
    dialogue: DialogueType,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    let mut user_registration_data_rx = user_registration_data_tx.subscribe();

    jobhandler_task_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();

    if user_registration_data_rx.recv().await.unwrap().is_none() {
        bot.send_message(msg.chat.id, NO_DB_MSG).await.unwrap();
        dialogue.exit().await.unwrap();
        return Ok(());
    }
    let argument = msg
        .text()
        .unwrap_or_default()
        .split_once(' ')
        .map(|slices| slices.1.trim());

    match parse_time_send_status_msgs(&bot, msg.chat.id, argument).await {
        Ok(parsed_time) => {
            jobhandler_task_tx
                .send(
                    UpdateRegistrationTask {
                        chat_id: msg.chat.id.0,
                        mensa_id: None,
                        hour: Some(parsed_time.hour),
                        minute: Some(parsed_time.minute),
                    }
                    .into(),
                )
                .unwrap();
        }

        Err(e) => {
            dbg!(e);
            dialogue
                .update(DialogueState::AwaitTimeReply)
                .await
                .unwrap();
        }
    }

    Ok(())
}

pub async fn reply_time_dialogue(
    bot: Bot,
    msg: Message,
    dialogue: DialogueType,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
) -> HandlerResult {
    if msg.text().is_none() {
        bot.send_message(
            msg.chat.id,
            "Das ist kein Text.\nBitte mit Uhrzeit antworten:",
        )
        .await?;
        return Ok(());
    }

    if let Ok(parsed_thing) =
        parse_time_send_status_msgs(&bot, msg.chat.id, Some(msg.text().unwrap())).await
    {
        dialogue.exit().await.unwrap();

        let registration_job = UpdateRegistrationTask {
            chat_id: msg.chat.id.0,
            mensa_id: None,
            hour: Some(parsed_thing.hour),
            minute: Some(parsed_thing.minute),
        }
        .into();
        jobhandler_task_tx.send(registration_job).unwrap();
    }

    Ok(())
}
