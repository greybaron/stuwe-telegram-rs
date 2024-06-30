use std::collections::BTreeMap;

use rusqlite::{params, Connection};

use crate::data_types::{Backend, JobHandlerTask, UpdateRegistrationTask, MM_DB, STUWE_DB};

pub fn update_db_row(data: &JobHandlerTask, backend: Backend) -> rusqlite::Result<()> {
    let sql_filename = match backend {
        Backend::MensiMates => MM_DB,
        Backend::StuWe => STUWE_DB,
    };
    let conn = Connection::open(sql_filename)?;

    // could be better but eh
    let mut update_mensa_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET mensa_id = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    let mut upd_hour_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET hour = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    let mut upd_min_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET minute = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    let mut upd_cb_stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET last_callback_id = ?2
            WHERE chat_id = ?1",
        )
        .unwrap();

    if let Some(mensa_id) = data.mensa_id {
        update_mensa_stmt.execute(params![data.chat_id, mensa_id])?;
    };

    if let Some(hour) = data.hour {
        upd_hour_stmt.execute(params![data.chat_id, hour])?;
    };

    if let Some(minute) = data.minute {
        upd_min_stmt.execute(params![data.chat_id, minute])?;
    };

    if let Some(callback_id) = data.callback_id {
        upd_cb_stmt.execute([data.chat_id, Some(callback_id.into())])?;
    };

    Ok(())
}

pub fn task_db_kill_auto(chat_id: i64, sql_filename: &str) -> rusqlite::Result<()> {
    let conn = Connection::open(sql_filename)?;
    let mut stmt = conn
        .prepare_cached(
            "UPDATE registrations
            SET hour = NULL, minute = NULL
            WHERE chat_id = ?1",
        )
        .unwrap();

    stmt.execute(params![chat_id])?;

    Ok(())
}

pub fn check_or_create_db_tables(sql_filename: &str) {
    let conn = Connection::open(sql_filename).unwrap();

    // ensure db table exists
    conn.prepare(
        "create table if not exists registrations (
        chat_id integer not null unique primary key,
        mensa_id integer not null,
        hour integer,
        minute integer,
        last_callback_id integer
        )",
    )
    .unwrap()
    .execute([])
    .unwrap();

    // table of all mensa names
    conn.prepare(
        "create table if not exists mensen (
            mensa_id integer primary key,
            mensa_name text not null unique
        )",
    )
    .unwrap()
    .execute([])
    .unwrap();

    // table of all meals
    conn.prepare(
        "create table if not exists meals (
            mensa_id integer,
            date text,
            json_text text,
            foreign key (mensa_id) references mensen(mensa_id)
        )",
    )
    .unwrap()
    .execute([])
    .unwrap();
}

pub fn init_mensa_id_db(sql_filename: &str, mensen: &BTreeMap<u8, String>) -> rusqlite::Result<()> {
    let conn = Connection::open(sql_filename)?;
    let mut stmt = conn
        .prepare_cached(
            "replace into mensen (mensa_id, mensa_name)
            values (?1, ?2)",
        )
        .unwrap();

    for (id, name) in mensen.iter() {
        stmt.execute(params![id.to_string(), name])?;
    }

    Ok(())
}

pub fn mensa_name_get_id_db(sql_filename: &str, mensa_name: &str) -> Option<u8> {
    let conn = Connection::open(sql_filename).unwrap();
    let mut stmt = conn
        .prepare_cached("SELECT (mensa_id) FROM mensen WHERE mensa_name = ?1")
        .unwrap();

    let mut id_iter = stmt.query_map([mensa_name], |row| row.get(0)).unwrap();

    id_iter.next().map(|row| row.unwrap())
}

pub fn get_all_user_registrations_db(sql_filename: &str) -> Vec<JobHandlerTask> {
    let mut tasks: Vec<JobHandlerTask> = Vec::new();
    let conn = Connection::open(sql_filename).unwrap();

    let mut stmt = conn
        .prepare_cached(
            "SELECT chat_id, mensa_id, hour, minute, last_callback_id FROM registrations",
        )
        .unwrap();

    let job_iter = stmt
        .query_map([], |row| {
            Ok(UpdateRegistrationTask {
                chat_id: row.get(0)?,
                mensa_id: row.get(1)?,
                hour: row.get(2)?,
                minute: row.get(3)?,
                callback_id: row.get(4)?,
            }
            .into())
        })
        .unwrap();

    for job in job_iter {
        tasks.push(job.unwrap())
    }

    tasks
}

pub fn init_db_record(
    job_handler_task: &JobHandlerTask,
    sql_filename: &str,
) -> rusqlite::Result<()> {
    let conn = Connection::open(sql_filename)?;
    let mut stmt = conn
        .prepare_cached(
            "replace into registrations (chat_id, mensa_id, hour, minute)
            values (?1, ?2, ?3, ?4)",
        )
        .unwrap();

    stmt.execute(params![
        job_handler_task.chat_id,
        job_handler_task.mensa_id,
        job_handler_task.hour.unwrap(),
        job_handler_task.minute.unwrap()
    ])?;
    Ok(())
}
