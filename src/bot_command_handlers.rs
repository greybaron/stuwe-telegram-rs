#![allow(unused_imports)]
use crate::data_backend::mm_parser::get_jwt_token;
use crate::data_types::{
    Backend, Command, DialogueState, HandlerResult, JobHandlerTask, JobHandlerTaskType, JobType,
    QueryRegistrationType, RegistrationEntry, MENSEN, MM_DB, NO_DB_MSG,
};
use crate::db_operations::{get_all_tasks_db, init_db_record, task_db_kill_auto, update_db_row};
use crate::shared_main::{
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

        registration_tx
            .send(make_query_data(msg.chat.id.0))
            .unwrap();
        let previous_markup_id = query_registration_rx.recv().await.unwrap().unwrap().4;

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
