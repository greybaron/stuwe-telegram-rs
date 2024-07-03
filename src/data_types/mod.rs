pub mod mm_data_types;
pub mod stuwe_data_types;

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
    DeleteRegistration,
    QueryRegistration,
    UpdateRegistration,
    BroadcastUpdate,
}

#[derive(Debug, Clone)]
pub struct JobHandlerTask {
    pub job_type: JobType,
    pub chat_id: Option<i64>,
    pub mensa_id: Option<u8>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
}

pub struct RegisterTask {
    pub chat_id: i64,
    pub mensa_id: u8,
    pub hour: u32,
    pub minute: u32,
}
impl From<RegisterTask> for JobHandlerTask {
    fn from(job: RegisterTask) -> Self {
        JobHandlerTask {
            job_type: JobType::Register,
            chat_id: Some(job.chat_id),
            mensa_id: Some(job.mensa_id),
            hour: Some(job.hour),
            minute: Some(job.minute),
        }
    }
}

pub struct UnregisterTask {
    pub chat_id: i64,
}
impl From<UnregisterTask> for JobHandlerTask {
    fn from(job: UnregisterTask) -> Self {
        JobHandlerTask {
            job_type: JobType::DeleteRegistration,
            chat_id: Some(job.chat_id),
            mensa_id: None,
            hour: None,
            minute: None,
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
        }
    }
}

pub struct UpdateRegistrationTask {
    pub chat_id: i64,
    pub mensa_id: Option<u8>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
}
impl From<UpdateRegistrationTask> for JobHandlerTask {
    fn from(job: UpdateRegistrationTask) -> Self {
        JobHandlerTask {
            job_type: JobType::UpdateRegistration,
            chat_id: Some(job.chat_id),
            mensa_id: job.mensa_id,
            hour: job.hour,
            minute: job.minute,
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
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct RegistrationEntry {
    pub job_uuid: Option<Uuid>,
    pub mensa_id: u8,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
}

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
#[derive(Debug, Clone)]
pub struct ParsedTimeAndLastMsgFromDialleougueue {
    pub hour: u32,
    pub minute: u32,
    pub msgid: Option<MessageId>,
}
