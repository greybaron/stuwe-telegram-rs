pub mod mm_data_types;
pub mod stuwe_data_types;

use teloxide::{
    dispatching::dialogue::InMemStorage, prelude::Dialogue, utils::command::BotCommands,
};
use thiserror::Error;
use uuid::Uuid;

pub const MENSEN: [(&str, u8); 9] = [
    ("Cafeteria Dittrichring", 153),
    ("Mensaria am Botanischen Garten", 127),
    ("Mensa Academica", 118),
    ("Mensa am Park", 106),
    ("Mensa am Elsterbecken", 115),
    ("Mensa am Medizincampus", 162),
    ("Mensa Peterssteinweg", 111),
    ("Mensa Schönauer Straße", 140),
    ("Mensa Tierklinik", 170),
];

pub const NO_DB_MSG: &str = "Bitte zuerst /start ausführen";

#[derive(BotCommands, Clone, Debug, PartialEq)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Gerichte für heute")]
    Heute,
    #[command(description = "Gerichte für morgen\n")]
    Morgen,
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

// // versions for mensimates (no meal update checking and broadcasting implemented)
// #[derive(Debug, Clone)]
// pub enum MMJobType {
//     Register,
//     Unregister,
//     QueryRegistration,
//     UpdateRegistration,
//     InsertCallbackMessageId,
// }

// #[derive(Debug, Clone)]
// pub struct MMJobHandlerTask {
//     pub job_type: MMJobType,
//     pub chat_id: Option<i64>,
//     pub mensa_id: Option<u8>,
//     pub hour: Option<u32>,
//     pub minute: Option<u32>,
//     pub callback_id: Option<i32>,
// }

// opt (job uuid), mensa id, opt(hour), opt(min), opt(callback message id)
pub type RegistrationEntry = (Option<Uuid>, u8, Option<u32>, Option<u32>, Option<i32>);

#[derive(Error, Debug, Clone)]
pub enum TimeParseError {
    #[error("Zeit konnte nicht gelesen werden")]
    InvalidTimePassed,
}
