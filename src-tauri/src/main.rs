// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Learn more about Tauri commands at https://tauri.app/v1/guides/features/command

use anyhow::Result;
use futures::TryStreamExt;
use rust_bert::pipelines::sentence_embeddings::{
    Embedding, SentenceEmbeddingsBuilder, SentenceEmbeddingsModelType,
};
use serde::{Deserialize, Serialize};
use sqlite_vec::sqlite3_vec_init;
use sqlx::{prelude::FromRow, sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::env;
use std::fmt::Debug;
use std::fs::OpenOptions;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use tauri::{App, Manager as _};
use tokio::{sync::oneshot, task};

struct AppState {
    db: Db,
    sentence_embedder: SentenceEmbedder,
}

#[tokio::main]
async fn main() {
    env::set_var("RUST_BACKTRACE", "1");

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            search_notes,
            add_note,
            get_notes,
            update_note,
            delete_note
        ])
        .build(tauri::generate_context!())
        .expect("error building the app");

    let db = setup_db(&app).await;

    let (_handle, sentence_embedder) = SentenceEmbedder::spawn();

    app.manage(AppState {
        db,
        sentence_embedder,
    });

    app.run(|_, _| {});
}

type Db = Pool<Sqlite>;

async fn setup_db(app: &App) -> Db {
    let mut path = app
        .path_resolver()
        .app_data_dir()
        .expect("could not get data_dir");

    println!("{:?}", path);

    // try to create application data directory if it doesn't exist
    match std::fs::create_dir_all(path.clone()) {
        Ok(_) => {}
        Err(err) => {
            panic!("error creating directory {}", err);
        }
    };

    path.push("db.sqlite");

    let result = OpenOptions::new().create_new(true).write(true).open(&path);

    match result {
        Ok(_) => println!("database file created"),
        Err(err) => match err.kind() {
            std::io::ErrorKind::AlreadyExists => println!("database file already exists"),
            _ => {
                panic!("error creating databse file {}", err);
            }
        },
    }

    unsafe {
        libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }

    let db = SqlitePoolOptions::new()
        .connect(path.to_str().unwrap())
        .await
        .unwrap();

    sqlx::migrate!("./migrations").run(&db).await.unwrap();

    let version: (String,) = sqlx::query_as("SELECT sqlite_version();")
        .fetch_one(&db)
        .await
        .unwrap();
    let vec_version: (String,) = sqlx::query_as("SELECT vec_version();")
        .fetch_one(&db)
        .await
        .unwrap();

    println!("sqlite version: {:?}", version);
    println!("vec version: {:?}", vec_version);

    // sqlx::query(
    //     "
    //     PRAGMA busy_timeout = 60000;
    //     PRAGMA journal_mode = WAL;
    // ",
    // )
    // .execute(&db)
    // .await
    // .unwrap();

    db
}

type Message = (Vec<String>, oneshot::Sender<Vec<Embedding>>);

/// Runner for Sentence Embedder
#[derive(Debug, Clone)]
pub struct SentenceEmbedder {
    sender: mpsc::SyncSender<Message>,
}

impl SentenceEmbedder {
    /// Spawn a embedder on a separate thread and return a embedder instance
    /// to interact with it
    pub fn spawn() -> (JoinHandle<Result<()>>, SentenceEmbedder) {
        let (sender, receiver) = mpsc::sync_channel(100);
        let handle = thread::spawn(move || Self::runner(receiver));
        (handle, SentenceEmbedder { sender })
    }

    /// The embedding runner itself
    fn runner(receiver: mpsc::Receiver<Message>) -> Result<()> {
        // Needs to be in sync runtime, async doesn't work
        let model = SentenceEmbeddingsBuilder::remote(SentenceEmbeddingsModelType::AllMiniLmL12V2)
            .create_model()
            .unwrap();

        while let Ok((texts, sender)) = receiver.recv() {
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            let embeddings = model.encode(&texts).unwrap();
            sender.send(embeddings).expect("sending embedding results");
        }

        Ok(())
    }

    /// Make the runner encode a sample and return the result
    pub async fn encode(&self, texts: Vec<String>) -> Result<Vec<Embedding>> {
        let (sender, receiver) = oneshot::channel();
        task::block_in_place(|| self.sender.send((texts, sender)))?;
        Ok(receiver.await?)
    }
}

#[derive(Debug, Serialize, Deserialize, FromRow)]
struct Note {
    id: u16,
    content: String,
    created_at: u32,
    updated_at: u32,
}

#[tauri::command]
async fn search_notes(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<Note>, String> {
    println!("search_notes invoked");
    let db = &state.db;
    let sentence_embedder = &state.sentence_embedder;
    let sentences = [query.clone()];
    let output = sentence_embedder.encode(sentences.to_vec()).await.unwrap();
    let embedding_json = serde_json::to_string(&output[0]).unwrap();

    println!("embedding_json: {}", embedding_json);

    let notes: Vec<Note> = sqlx::query_as::<_, Note>(
        r#"
    WITH matches as (
        SELECT
            rowid,
            distance
        FROM vec_notes
        WHERE content_embedding match (?1)
        ORDER BY distance
        limit 10
    )
    SELECT 
        notes.id,
        notes.content,
        notes.created_at,
        notes.updated_at
    FROM matches
    LEFT JOIN notes on notes.rowid = matches.rowid"#,
    )
    .bind(embedding_json)
    .fetch(db)
    .try_collect()
    .await
    .map_err(|e| format!("Failed to search notes {}", e))?;

    Ok(notes)
}

#[tauri::command]
async fn add_note(state: tauri::State<'_, AppState>, content: String) -> Result<(), String> {
    println!("add_note invoked");

    let sentence_embedder = &state.sentence_embedder;

    let sentences = [content.clone()];

    println!("sentences: {:?}", sentences.to_vec());

    let output = sentence_embedder.encode(sentences.to_vec()).await.unwrap();

    let embedding_json = serde_json::to_string(&output[0]).unwrap();

    println!("embedding_json: {}", embedding_json);

    let db = &state.db;

    let inserted_note =
        sqlx::query("INSERT INTO notes (content, content_embedding) VALUES (?1, ?2)")
            .bind(content)
            .bind(embedding_json.as_str())
            .execute(db)
            .await
            .unwrap();

    println!("inserted_note : {:?}", inserted_note);

    let inserted_note_row_id = inserted_note.last_insert_rowid();

    let inserted_vec_note = sqlx::query(
        r#"
    WITH note AS (
        SELECT rowid, content_embedding
        FROM notes
        WHERE "notes"."rowid" = ?1
    )
    INSERT INTO vec_notes (rowid, content_embedding)
    VALUES (?2, ?3)
    "#,
    )
    .bind(inserted_note_row_id)
    .bind(inserted_note_row_id)
    .bind(embedding_json)
    .execute(db)
    .await
    .unwrap();

    println!("inserted_vec_note : {:?}", inserted_vec_note);

    Ok(())
}

#[tauri::command]
async fn get_notes(state: tauri::State<'_, AppState>) -> Result<Vec<Note>, String> {
    let db = &state.db;
    let notes: Vec<Note> = sqlx::query_as::<_, Note>("SELECT * FROM notes")
        .fetch(db)
        .try_collect()
        .await
        .map_err(|e| format!("Failed to get notes {}", e))?;

    Ok(notes)
}

#[tauri::command]
async fn update_note(state: tauri::State<'_, AppState>, note: Note) -> Result<(), String> {
    let db = &state.db;

    sqlx::query("UPDATE notes SET content = ?1 WHERE id = ?3")
        .bind(note.content)
        .bind(note.id)
        .execute(db)
        .await
        .map_err(|e| format!("could not update note {}", e))?;

    Ok(())
}

#[tauri::command]
async fn delete_note(state: tauri::State<'_, AppState>, id: u16) -> Result<(), String> {
    let db = &state.db;

    sqlx::query("DELETE FROM notes WHERE id = ?1")
        .bind(id)
        .execute(db)
        .await
        .map_err(|e| format!("could not delete note {}", e))?;

    Ok(())
}
