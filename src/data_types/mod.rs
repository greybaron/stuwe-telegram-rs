pub mod mm_data_types;
pub mod stuwe_data_types;

use std::sync::OnceLock;

use teloxide::{
    dispatching::dialogue::InMemStorage, prelude::Dialogue, types::MessageId,
    utils::command::BotCommands,
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

pub static OLLAMA_HOST: OnceLock<Option<String>> = OnceLock::new();

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

pub struct RegisterTask {
    pub chat_id: i64,
    pub mensa_id: u8,
    pub hour: u32,
    pub minute: u32,
    pub callback_id: i32,
}
impl From<RegisterTask> for JobHandlerTask {
    fn from(job: RegisterTask) -> Self {
        JobHandlerTask {
            job_type: JobType::Register,
            chat_id: Some(job.chat_id),
            mensa_id: Some(job.mensa_id),
            hour: Some(job.hour),
            minute: Some(job.minute),
            callback_id: Some(job.callback_id),
        }
    }
}

pub struct UnregisterTask {
    pub chat_id: i64,
}
impl From<UnregisterTask> for JobHandlerTask {
    fn from(job: UnregisterTask) -> Self {
        JobHandlerTask {
            job_type: JobType::Unregister,
            chat_id: Some(job.chat_id),
            mensa_id: None,
            hour: None,
            minute: None,
            callback_id: None,
        }
    }
}

pub struct QueryRegistrationTask {
    pub chat_id: i64,
}
impl From<QueryRegistrationTask> for JobHandlerTask {
    fn from(job: QueryRegistrationTask) -> Self {
        JobHandlerTask {
            job_type: JobType::QueryRegistration,
            chat_id: Some(job.chat_id),
            mensa_id: None,
            hour: None,
            minute: None,
            callback_id: None,
        }
    }
}

pub struct UpdateRegistrationTask {
    pub chat_id: i64,
    pub mensa_id: Option<u8>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    pub callback_id: Option<i32>,
}
impl From<UpdateRegistrationTask> for JobHandlerTask {
    fn from(job: UpdateRegistrationTask) -> Self {
        JobHandlerTask {
            job_type: JobType::UpdateRegistration,
            chat_id: Some(job.chat_id),
            mensa_id: job.mensa_id,
            hour: job.hour,
            minute: job.minute,
            callback_id: job.callback_id,
        }
    }
}

pub struct BroadcastUpdateTask {
    pub mensa_id: u8,
}
impl From<BroadcastUpdateTask> for JobHandlerTask {
    fn from(job: BroadcastUpdateTask) -> Self {
        JobHandlerTask {
            job_type: JobType::BroadcastUpdate,
            chat_id: None,
            mensa_id: Some(job.mensa_id),
            hour: None,
            minute: None,
            callback_id: None,
        }
    }
}

pub struct InsertMarkupMessageIDTask {
    pub chat_id: i64,
    pub callback_id: i32,
}
impl From<InsertMarkupMessageIDTask> for JobHandlerTask {
    fn from(job: InsertMarkupMessageIDTask) -> Self {
        JobHandlerTask {
            job_type: JobType::InsertMarkupMessageID,
            chat_id: Some(job.chat_id),
            mensa_id: None,
            hour: None,
            minute: None,
            callback_id: Some(job.callback_id),
        }
    }
}

// opt (job uuid), mensa id, opt(hour), opt(min), opt(callback message id)
pub type RegistrationEntry = (Option<Uuid>, u8, Option<u32>, Option<u32>, Option<i32>);

#[derive(Error, Debug, Clone)]
pub enum TimeParseError {
    #[error("Zeit konnte nicht gelesen werden")]
    InvalidTimePassed,
    #[error("Ollama API ist nicht erreichbar")]
    OllamaUnavailable,
    #[error("Ollama API ist nicht konfiguriert")]
    OllamaUnconfigured,
    #[error("Kein optionaler Zeitparam. bei Dialogstart")]
    NoTimeArgPassed,
}

pub struct ParsedTimeAndLastMsgFromDialleougueue {
    pub hour: u32,
    pub minute: u32,
    pub msgid: Option<MessageId>,
}

// impl From<reqwest::Error> for TimeParseError {
//     fn from(e: reqwest::Error) -> Self {
//         TimeParseError::Llama3Error(e.to_string())
//     }
// }
