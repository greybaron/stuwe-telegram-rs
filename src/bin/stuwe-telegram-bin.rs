cfg_if::cfg_if! {
    if #[cfg(feature = "campusdual")] {
        // GEANT_OV_RSA_CA_4_tcs-cert3.pem has to be properly set up in /etc/ssl/certs
        use stuwe_telegram_rs::data_types::stuwe_data_types::CampusDualData;
    }
}

use stuwe_telegram_rs::bot_command_handlers::{
    change_mensa, day_cmd, reply_time_dialogue, show_different_mensa, start, start_time_dialogue,
    subscribe, unsubscribe,
};
use stuwe_telegram_rs::data_backend::stuwe_parser::{get_mensen, update_cache};
use stuwe_telegram_rs::data_types::{
    Backend, Command, DialogueState, HandlerResult, JobHandlerTask, JobHandlerTaskType, JobType,
    QueryRegistrationType, RegistrationEntry, OLLAMA_HOST, OLLAMA_MODEL, STUWE_DB,
};
use stuwe_telegram_rs::db_operations::{check_or_create_db_tables, init_mensa_id_db};
use stuwe_telegram_rs::shared_main::{callback_handler, logger_init};
use stuwe_telegram_rs::task_scheduler_funcs::{
    db_insert_markup_id, handle_add_registration_task, handle_broadcast_update_task,
    handle_delete_registration_task, handle_query_registration_task,
    handle_update_registration_task, load_jobs_from_db, start_mensacache_and_campusdual_job,
};

use clap::Parser;
use log::log_enabled;
use std::{collections::BTreeMap, sync::Arc};
use teloxide::{
    dispatching::{
        dialogue::{self, InMemStorage},
        UpdateHandler,
    },
    prelude::*,
};
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::JobScheduler;

#[cfg(feature = "campusdual")]
/// Telegram bot to receive daily meal plans from any Studentenwerk Leipzig mensa.
/// {n}CampusDual support is enabled.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// The telegram bot token to be used
    #[arg(short, long, env)]
    token: String,
    #[arg(short, long, env = "CD_USER", id = "CAMPUSDUAL-USER")]
    user: String,
    #[arg(short, long, env = "CD_PASSWORD", id = "CD-PASSWORD")]
    password: String,
    /// The Chat-ID which will receive CampusDual exam scores
    #[arg(short, long, env)]
    chatid: i64,
    /// Enable verbose logging (mostly performance metrics){n}[SETS env: RUST_LOG=debug]
    #[arg(short, long)]
    verbose: bool,
    /// Ollama API host for AI time parsing bloatware{n}Example: <http://127.0.0.1:11434/api>
    #[arg(long, env = "OLLAMA_HOST")]
    ollama_host: Option<String>,
    /// Ollama model for inference{n}Example: 'llama3:latest'
    #[arg(long, env = "OLLAMA_MODEL")]
    ollama_model: Option<String>,
}
#[cfg(not(feature = "campusdual"))]
/// Telegram bot to receive daily meal plans from any Studentenwerk Leipzig mensa.
/// {n}CampusDual support is disabled.
#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    /// The telegram bot token to be used
    #[arg(short, long, env)]
    token: String,
    /// enable verbose logging (mostly performance metrics){n}[SETS env: RUST_LOG=debug]
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() {
    //// Args setup
    let args = Args::parse();
    OLLAMA_HOST.get_or_init(|| args.ollama_host);
    OLLAMA_MODEL.get_or_init(|| args.ollama_model);

    if args.verbose {
        std::env::set_var("RUST_LOG", "debug");
    }

    logger_init(module_path!());
    log::info!("Starting bot...");

    if !(log_enabled!(log::Level::Debug) || log_enabled!(log::Level::Trace)) {
        log::info!("Enable verbose logging for performance metrics");
    }

    #[cfg(feature = "campusdual")]
    let cd_data = CampusDualData {
        username: args.user,
        password: args.password,
        chat_id: args.chatid,
    };

    //// DB setup
    check_or_create_db_tables(STUWE_DB);

    let mensen = get_mensen().await;
    init_mensa_id_db(STUWE_DB, &mensen).unwrap();

    // always update cache on startup
    log::info!(target: "stuwe_telegram_rs::TaskSched", "Updating cache...");
    match update_cache().await {
        Ok(_) => log::info!(target: "stuwe_telegram_rs::TaskSched", "Cache updated!"),
        Err(e) => log::error!(target: "stuwe_telegram_rs::TaskSched", "Cache update failed: {}", e),
    }

    let bot = Bot::new(args.token);

    let (jobhandler_task_tx, jobhandler_task_rx): JobHandlerTaskType = broadcast::channel(10);
    let (user_registration_data_tx, _): QueryRegistrationType = broadcast::channel(10);

    // every user has a mensa_id, but only users with auto send have a job_uuid inside RegistrEntry
    {
        let bot = bot.clone();
        // there is effectively only one tx and rx, however since rx cant be passed as dptree dep (?!),
        // tx has to be cloned and passed to both (inside command_handler it will be resubscribed to rx)
        let jobhandler_task_tx = jobhandler_task_tx.clone();
        let user_registration_data_tx = user_registration_data_tx.clone();
        tokio::spawn(async move {
            log::info!("Starting task scheduler...");
            run_task_scheduler(
                bot,
                jobhandler_task_tx,
                jobhandler_task_rx,
                user_registration_data_tx,
                #[cfg(feature = "campusdual")]
                cd_data,
            )
            .await;
        });
    }

    // passing a receiver doesnt work for some reason, so sending user_registration_data_tx and resubscribing to get rx
    let command_handler_deps = dptree::deps![
        InMemStorage::<DialogueState>::new(),
        mensen,
        jobhandler_task_tx,
        user_registration_data_tx,
        Backend::StuWe,
        None::<Arc<RwLock<String>>>
    ];
    Dispatcher::builder(bot, schema())
        .dependencies(command_handler_deps)
        // .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

fn schema() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    use dptree::case;

    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(dptree::case![Command::Start].endpoint(start))
        .branch(dptree::case![Command::Heute].endpoint(day_cmd))
        .branch(dptree::case![Command::Morgen].endpoint(day_cmd))
        .branch(dptree::case![Command::Uebermorgen].endpoint(day_cmd))
        .branch(dptree::case![Command::Andere].endpoint(show_different_mensa))
        .branch(dptree::case![Command::Subscribe].endpoint(subscribe))
        .branch(dptree::case![Command::Unsubscribe].endpoint(unsubscribe))
        .branch(dptree::case![Command::Mensa].endpoint(change_mensa))
        .branch(dptree::case![Command::Uhrzeit].endpoint(start_time_dialogue));

    let message_handler = Update::filter_message()
        .branch(command_handler)
        .branch(case![DialogueState::AwaitTimeReply].endpoint(reply_time_dialogue))
        .branch(dptree::endpoint(invalid_cmd));

    let callback_query_handler = Update::filter_callback_query().endpoint(callback_handler);

    dialogue::enter::<Update, InMemStorage<DialogueState>, DialogueState, _>()
        .branch(message_handler)
        .branch(callback_query_handler)
}

async fn invalid_cmd(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, "Das ist kein Befehl.")
        .await?;
    Ok(())
}

async fn run_task_scheduler(
    bot: Bot,
    jobhandler_task_tx: broadcast::Sender<JobHandlerTask>,
    mut jobhandler_task_rx: broadcast::Receiver<JobHandlerTask>,
    user_registration_data_tx: broadcast::Sender<Option<RegistrationEntry>>,
    #[cfg(feature = "campusdual")] cd_data: CampusDualData,
) {
    let sched = JobScheduler::new().await.unwrap();

    start_mensacache_and_campusdual_job(bot.clone(), &sched, jobhandler_task_tx.clone(), cd_data)
        .await;

    let mut loaded_user_data: BTreeMap<i64, RegistrationEntry> = BTreeMap::new();
    load_jobs_from_db(
        &bot,
        &sched,
        &mut loaded_user_data,
        jobhandler_task_tx.clone(),
        user_registration_data_tx.clone(),
    )
    .await;

    // start scheduler (non blocking)
    sched.start().await.unwrap();

    log::info!(target: "stuwe_telegram_rs::TaskSched", "Ready.");

    // receive job update msg (register/unregister/check existence)
    while let Ok(job_handler_task) = jobhandler_task_rx.recv().await {
        match job_handler_task.job_type {
            JobType::Register => {
                handle_add_registration_task(
                    &bot,
                    job_handler_task,
                    &sched,
                    &mut loaded_user_data,
                    jobhandler_task_tx.clone(),
                    user_registration_data_tx.subscribe(),
                )
                .await;
            }

            JobType::UpdateRegistration => {
                handle_update_registration_task(
                    &bot,
                    job_handler_task,
                    &sched,
                    &mut loaded_user_data,
                    jobhandler_task_tx.clone(),
                    user_registration_data_tx.subscribe(),
                )
                .await;
            }

            JobType::DeleteRegistration => {
                handle_delete_registration_task(job_handler_task, &sched, &mut loaded_user_data)
                    .await;
            }

            JobType::QueryRegistration => {
                handle_query_registration_task(
                    job_handler_task,
                    &mut loaded_user_data,
                    user_registration_data_tx.clone(),
                )
                .await;
            }

            JobType::BroadcastUpdate => {
                // let test = job_handler_tx;
                handle_broadcast_update_task(
                    &bot,
                    job_handler_task,
                    &mut loaded_user_data,
                    jobhandler_task_tx.clone(),
                )
                .await;
            }

            JobType::InsertMarkupMessageID => {
                db_insert_markup_id(job_handler_task, &mut loaded_user_data).await;
            }
        }
    }
}
