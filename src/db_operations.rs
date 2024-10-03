use rusqlite::{params, Connection};
use std::{collections::BTreeMap, process::exit};

use crate::{
    constants::DB_FILENAME,
    data_types::{JobHandlerTask, UpdateRegistrationTask},
};

pub fn update_db_row(data: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open(DB_FILENAME.get().unwrap())?;

    // could be better but eh
    let mut update_mensa_stmt = conn.prepare_cached(
        "UPDATE registrations
            SET mensa_id = ?2
            WHERE chat_id = ?1",
    )?;

    let mut upd_hour_stmt = conn.prepare_cached(
        "UPDATE registrations
            SET hour = ?2
            WHERE chat_id = ?1",
    )?;

    let mut upd_min_stmt = conn.prepare_cached(
        "UPDATE registrations
            SET minute = ?2
            WHERE chat_id = ?1",
    )?;

    if let Some(mensa_id) = data.mensa_id {
        update_mensa_stmt.execute(params![data.chat_id, mensa_id])?;
    };

    if let Some(hour) = data.hour {
        upd_hour_stmt.execute(params![data.chat_id, hour])?;
    };

    if let Some(minute) = data.minute {
        upd_min_stmt.execute(params![data.chat_id, minute])?;
    };

    Ok(())
}

pub fn task_db_kill_auto(chat_id: i64) -> rusqlite::Result<()> {
    let conn = Connection::open(DB_FILENAME.get().unwrap())?;
    let mut stmt = conn.prepare_cached(
        "UPDATE registrations
            SET hour = NULL, minute = NULL
            WHERE chat_id = ?1",
    )?;

    stmt.execute(params![chat_id])?;

    Ok(())
}

pub fn check_or_create_db_tables() -> rusqlite::Result<()> {
    if DB_FILENAME.get().is_none() {
        log::error!("DB_FILENAME not set");
        exit(1);
    }
    let conn = Connection::open(DB_FILENAME.get().unwrap())?;

    // ensure db table exists
    conn.prepare(
        "create table if not exists registrations (
        chat_id integer not null unique primary key,
        mensa_id integer not null,
        hour integer,
        minute integer
        )",
    )?
    .execute([])?;

    // table of all mensa names
    conn.prepare(
        "create table if not exists mensen (
            mensa_id integer primary key,
            mensa_name text not null unique
        )",
    )?
    .execute([])?;

    // table of all meals
    conn.prepare(
        "create table if not exists meals (
            mensa_id integer,
            date text,
            json_text text,
            foreign key (mensa_id) references mensen(mensa_id)
        )",
    )?
    .execute([])?;

    Ok(())
}

pub fn init_mensa_id_db(mensen: &BTreeMap<u8, String>) -> rusqlite::Result<()> {
    let conn = Connection::open(DB_FILENAME.get().unwrap())?;
    let mut stmt = conn.prepare_cached(
        "replace into mensen (mensa_id, mensa_name)
            values (?1, ?2)",
    )?;

    for (id, name) in mensen.iter() {
        stmt.execute(params![id.to_string(), name])?;
    }

    Ok(())
}

pub fn get_all_user_registrations_db() -> rusqlite::Result<Vec<JobHandlerTask>> {
    let mut tasks: Vec<JobHandlerTask> = Vec::new();
    let conn = Connection::open(DB_FILENAME.get().unwrap()).unwrap();

    let mut stmt =
        conn.prepare_cached("SELECT chat_id, mensa_id, hour, minute FROM registrations")?;

    let job_iter = stmt.query_map([], |row| {
        Ok(UpdateRegistrationTask {
            chat_id: row.get(0)?,
            mensa_id: row.get(1)?,
            hour: row.get(2)?,
            minute: row.get(3)?,
        }
        .into())
    })?;

    for job in job_iter {
        tasks.push(job.unwrap())
    }

    Ok(tasks)
}

pub fn init_db_record(job_handler_task: &JobHandlerTask) -> rusqlite::Result<()> {
    let conn = Connection::open(DB_FILENAME.get().unwrap())?;
    let mut stmt = conn.prepare_cached(
        "replace into registrations (chat_id, mensa_id, hour, minute)
            values (?1, ?2, ?3, ?4)",
    )?;

    stmt.execute(params![
        job_handler_task.chat_id,
        job_handler_task.mensa_id,
        job_handler_task.hour.unwrap(),
        job_handler_task.minute.unwrap()
    ])?;

    Ok(())
}
