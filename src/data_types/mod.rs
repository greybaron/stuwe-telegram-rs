pub mod stuwe_data_types;
pub mod mm_data_types;

use thiserror::Error;
use uuid::Uuid;

// used internally for teloxide/Telegram bot
#[derive(Debug, Clone)]
pub enum JobType {
    Register,
    Unregister,
    QueryRegistration,
    UpdateRegistration,
    BroadcastUpdate,
    InsertCallbackMessageId,
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
pub type RegistrationEntry = (
    Option<Uuid>,
    u8,
    Option<u32>,
    Option<u32>,
    Option<i32>,
);

#[derive(Error, Debug, Clone)]
pub enum TimeParseError {
    #[error("Keine Zeit übergeben")]
    NoTimePassed,
    #[error("Zeit konnte nicht gelesen werden")]
    InvalidTimePassed,
}