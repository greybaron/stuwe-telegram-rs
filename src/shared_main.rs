use std::collections::BTreeMap;

use regex_lite::Regex;
use static_init::dynamic;
use teloxide::prelude::*;
use teloxide_core::{
    types::{InlineKeyboardButton, InlineKeyboardMarkup, Message},
    Bot,
};
use tokio::sync::broadcast;

use crate::data_types::{
    DialogueState, DialogueType, HandlerResult, JobHandlerTask, JobType, RegistrationEntry,
    TimeParseError,
};

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
    if query_registration_rx.recv().await.unwrap().is_some() {
        bot.send_message(msg.chat.id, "Bitte mit Uhrzeit antworten:")
            .await
            .unwrap();
        dialogue
            .update(DialogueState::AwaitTimeReply)
            .await
            .unwrap();
    } else {
        bot.send_message(msg.chat.id, no_db_message).await.unwrap();
        dialogue.exit().await.unwrap();
    }
    Ok(())
}

pub fn parse_time(txt: &str) -> Result<(u32, u32), TimeParseError> {
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

pub fn make_days_keyboard() -> InlineKeyboardMarkup {
    let keyboard = vec![vec![
        InlineKeyboardButton::callback("Heute", "day:0"),
        InlineKeyboardButton::callback("Morgen", "day:1"),
        InlineKeyboardButton::callback("Überm.", "day:2"),
    ]];
    InlineKeyboardMarkup::new(keyboard)
}
