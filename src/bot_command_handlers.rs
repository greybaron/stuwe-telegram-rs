#![allow(unused_imports)]
use crate::data_backend::mm_parser::get_jwt_token;
use crate::data_types::{
    Backend, Command, DialogueState, DialogueType, HandlerResult, InsertMarkupMessageIDTask,
    JobHandlerTask, JobHandlerTaskType, MensaKeyboardAction, ParsedTimeAndLastMsgFromDialleougueue,
    QueryRegistrationType, RegistrationEntry, TimeParseError, UnregisterTask,
    UpdateRegistrationTask, MM_DB, NO_DB_MSG, OLLAMA_HOST,
};
use crate::db_operations::{
    get_all_user_registrations_db, init_db_record, task_db_kill_auto, update_db_row,
};
use crate::shared_main::{
    build_meal_message_dispatcher, callback_handler, load_job, logger_init, make_days_keyboard,
    make_mensa_keyboard, make_query_data,
};

use chrono::{Datelike, Timelike};
use clap::Parser;
use log::log_enabled;
use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use static_init::dynamic;
use std::env::args;
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

pub async fn start(bot: Bot, msg: Message, mensen: BTreeMap<u8, String>) -> HandlerResult {
    let keyboard = make_mensa_keyboard(mensen, MensaKeyboardAction::Register);
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
        let task = InsertMarkupMessageIDTask {
            chat_id: msg.chat.id.0,
            callback_id: markup_id,
        }
        .into();

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

            let registration_job = UpdateRegistrationTask {
                chat_id: msg.chat.id.0,
                mensa_id: None,
                hour: Some(6),
                minute: Some(0),
                callback_id: None,
            }
            .into();

            registration_tx.send(registration_job).unwrap();
        }
    } else {
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }

    Ok(())
}

async fn mensa_disp_or_upd(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<u8, String>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
    disp_or_update: MensaKeyboardAction,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

    registration_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();
    if query_registration_rx.recv().await.unwrap().is_some() {
        let keyboard = make_mensa_keyboard(mensen, disp_or_update);
        bot.send_message(msg.chat.id, "Mensa auswählen:")
            .reply_markup(keyboard)
            .await?;
    } else {
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
    }
    Ok(())
}

pub async fn change_mensa(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<u8, String>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    mensa_disp_or_upd(
        bot,
        msg,
        mensen,
        registration_tx,
        query_registration_tx,
        MensaKeyboardAction::Update,
    )
    .await
}

pub async fn show_different_mensa(
    bot: Bot,
    msg: Message,
    mensen: BTreeMap<u8, String>,
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    mensa_disp_or_upd(
        bot,
        msg,
        mensen,
        registration_tx,
        query_registration_tx,
        MensaKeyboardAction::DisplayOnce,
    )
    .await
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

    if query_registration_rx.recv().await.unwrap().is_none() {
        bot.send_message(msg.chat.id, NO_DB_MSG).await.unwrap();
        dialogue.exit().await.unwrap();
        return Ok(());
    }
    let argument = msg
        .text()
        .unwrap_or_default()
        .split_once(' ')
        .map(|slices| slices.1.trim());

    let parsed_time = match argument {
        Some(arg) => parse_time_send_status_msgs(&bot, msg.chat.id, arg).await,
        None => Err(TimeParseError::NoTimeArgPassed),
    };

    match parsed_time {
        Ok(parsed_thing) => {
            let registration_job = UpdateRegistrationTask {
                chat_id: msg.chat.id.0,
                mensa_id: None,
                hour: Some(parsed_thing.hour),
                minute: Some(parsed_thing.minute),
                callback_id: None,
            }
            .into();

            registration_tx.send(registration_job).unwrap();
        }
        Err(TimeParseError::NoTimeArgPassed) => {
            bot.send_message(msg.chat.id, "Bitte mit Uhrzeit antworten:")
                .await
                .unwrap();

            dialogue
                .update(DialogueState::AwaitTimeReply)
                .await
                .unwrap();
        }
        Err(_) => {
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
    registration_tx: broadcast::Sender<JobHandlerTask>,
    query_registration_tx: broadcast::Sender<Option<RegistrationEntry>>,
) -> HandlerResult {
    let mut query_registration_rx = query_registration_tx.subscribe();

    registration_tx
        .send(make_query_data(msg.chat.id.0))
        .unwrap();

    if query_registration_rx.recv().await.unwrap().is_none() {
        bot.send_message(msg.chat.id, NO_DB_MSG).await?;
        dialogue.exit().await.unwrap();

        return Ok(());
    }

    if msg.text().is_none() {
        bot.send_message(
            msg.chat.id,
            "Das ist kein Text.\nBitte mit Uhrzeit antworten:",
        )
        .await?;
        return Ok(());
    }

    if let Ok(parsed_thing) =
        parse_time_send_status_msgs(&bot, msg.chat.id, msg.text().unwrap()).await
    {
        dialogue.exit().await.unwrap();

        let registration_job = UpdateRegistrationTask {
            chat_id: msg.chat.id.0,
            mensa_id: None,
            hour: Some(parsed_thing.hour),
            minute: Some(parsed_thing.minute),
            callback_id: None,
        }
        .into();
        registration_tx.send(registration_job).unwrap();
    }

    Ok(())
}

async fn parse_time_send_status_msgs(
    bot: &Bot,
    chatid: ChatId,
    txt: &str,
) -> Result<ParsedTimeAndLastMsgFromDialleougueue, TimeParseError> {
    if let Ok((hour, minute)) = rex_parse_time(txt) {
        let msgtxt = format!(
            "Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr \
        gesendet.\n\n/unsubscribe zum Deaktivieren",
            hour, minute
        );

        bot.send_message(chatid, msgtxt).await.unwrap();

        Ok(ParsedTimeAndLastMsgFromDialleougueue {
            hour,
            minute,
            msgid: None,
        })
    } else {
        // try ai
        let oracle_msgid = bot
            .send_message(chatid, "Das Orakel wird befragt...")
            .await
            .unwrap()
            .id;

        let parse_result = ai_parse_time(txt).await;

        match parse_result {
            Ok(parsed) => {
                let msgtxt = format!(
                    "Plan wird ab jetzt automatisch an Wochentagen {:02}:{:02} Uhr \
                gesendet.\n\n/unsubscribe zum Deaktivieren",
                    parsed.0, parsed.1
                );

                bot.edit_message_text(chatid, oracle_msgid, msgtxt)
                    .await
                    .unwrap();

                Ok(ParsedTimeAndLastMsgFromDialleougueue {
                    hour: parsed.0,
                    minute: parsed.1,
                    msgid: Some(oracle_msgid),
                })
            }
            // Err(TimeParseError::NoTimeArgPassed)
            Err(TimeParseError::InvalidTimePassed) => {
                bot.edit_message_text(chatid, oracle_msgid, "Das Orakel verharrt in Ungewissheit.")
                    .await
                    .unwrap();

                Err(TimeParseError::InvalidTimePassed)
            }
            Err(TimeParseError::OllamaUnavailable) => {
                bot.edit_message_text(
                    chatid,
                    oracle_msgid,
                    "Das Orakel erhört unsere Bitten nicht.",
                )
                .await
                .unwrap();

                Err(TimeParseError::OllamaUnavailable)
            }
            Err(TimeParseError::OllamaUnconfigured) => {
                bot.edit_message_text(
                    chatid,
                    oracle_msgid,
                    "Eingegebene Zeit ist ungültig.\nBitte mit Uhrzeit antworten:",
                )
                .await
                .unwrap();

                Err(TimeParseError::OllamaUnconfigured)
            }
            Err(TimeParseError::NoTimeArgPassed) => {
                unreachable!()
            }
        }
    }
}
async fn ai_parse_time(txt: &str) -> Result<(u32, u32), TimeParseError> {
    let prompt = format!(
         "Finde die Zeitangabe. Antworte NUR 'HH:mm' ODER 'nicht vorhanden'. KEINE ERKLÄRUNG. Eingabe: #{}#",
    txt
        );

    let client = reqwest::Client::new();
    let params = json!(
        {
            "model": "llama3:latest",
            "prompt": &prompt,
            "stream": false
        }
    );

    let ollama_host = OLLAMA_HOST.get().unwrap();

    if ollama_host.is_none() {
        log::warn!("Ollama API is unconfigured, cannot fancy-parse time");
        return Err(TimeParseError::OllamaUnconfigured);
    }

    log::info!("AI Query: '{}'", prompt);

    let res = client
        .post(format!("{}/generate", ollama_host.as_ref().unwrap()))
        .body(params.to_string())
        .send()
        .await;

    if let Ok(res) = res {
        let txt = res.text().await.unwrap();

        #[derive(Serialize, Deserialize, Debug)]
        struct LlamaResponse {
            response: String,
            done: bool,
            context: Vec<i64>,
        }

        let struct_ai_resp: LlamaResponse = serde_json::from_str(&txt).unwrap();

        log::info!("AI Response: '{}'", struct_ai_resp.response);
        rex_parse_time(&struct_ai_resp.response)
    } else {
        log::warn!("Ollama API unavailable: {}", res.err().unwrap());
        Err(TimeParseError::OllamaUnavailable)
    }
}

fn rex_parse_time(txt: &str) -> Result<(u32, u32), TimeParseError> {
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
