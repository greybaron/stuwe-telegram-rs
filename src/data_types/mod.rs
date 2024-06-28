pub mod mm_data_types;
pub mod stuwe_data_types;

use teloxide::{
    dispatching::dialogue::InMemStorage, prelude::Dialogue, utils::command::BotCommands,
};
use thiserror::Error;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Copy, Clone)]
pub enum Backend {
    MensiMates,
    StuWe,
}

pub const NO_DB_MSG: &str = "Bitte zuerst /start ausführen";
pub const MM_DB: &str = "mensimates.sqlite";
pub const STUWE_DB: &str = "stuwe.sqlite";

#[derive(BotCommands, Clone, Debug, PartialEq)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Gerichte für heute")]
    Heute,
    #[command(description = "Gerichte für morgen\n")]
    Morgen,
    #[command(description = "Andere Mensa abrufen\n")]
    Andere,
    #[command(description = "off")]
    Uebermorgen,
    #[command(description = "automat. Nachrichten AN")]
    Subscribe,
    #[command(description = "autom. Nachrichten AUS")]
    Unsubscribe,
    #[command(description = "off")]
    Uhrzeit,
    #[command(description = "off")]
    Mensa,
    #[command(description = "off")]
    Start,
}

#[derive(Clone, Default)]
pub enum DialogueState {
    #[default]
    Default,
    AwaitTimeReply,
}

pub type DialogueType = Dialogue<DialogueState, InMemStorage<DialogueState>>;
pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

pub type JobHandlerTaskType = (
    broadcast::Sender<JobHandlerTask>,
    broadcast::Receiver<JobHandlerTask>,
);

pub type QueryRegistrationType = (
    broadcast::Sender<Option<RegistrationEntry>>,
    broadcast::Receiver<Option<RegistrationEntry>>,
);

pub enum MensaKeyboardAction {
    Register,
    Update,
    DisplayOnce,
}

// used internally for teloxide/Telegram bot
#[derive(Debug, Clone)]
pub enum JobType {
    Register,
    Unregister,
    QueryRegistration,
    UpdateRegistration,
    BroadcastUpdate,
    InsertMarkupMessageID,
}

#[derive(Debug, Clone)]
pub struct JobHandlerTask {
    pub job_type: JobType,
    pub chat_id: Option<i64>,
    pub mensa_id: Option<u8>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    pub callback_id: Option<i32>,
}

// opt (job uuid), mensa id, opt(hour), opt(min), opt(callback message id)
pub type RegistrationEntry = (Option<Uuid>, u8, Option<u32>, Option<u32>, Option<i32>);

#[derive(Error, Debug, Clone)]
pub enum TimeParseError {
    #[error("Zeit konnte nicht gelesen werden")]
    InvalidTimePassed,
    //     #[error("llama3-Error: {0}")]
    //     Llama3Error(String),
}

// impl From<reqwest::Error> for TimeParseError {
//     fn from(e: reqwest::Error) -> Self {
//         TimeParseError::Llama3Error(e.to_string())
//     }
// }
