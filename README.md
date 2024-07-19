An issue with sqlite-vec, where running an insert to the vector virtual table seems to cause it to get stuck.

This occurs when:

1. Running an insert after freshly creating a database.
2. Running a second insert after an insert during the connection.

## Reproduce

Need two terminal windows because I haven't figured out a way to to get the path for the sentence embedding model with Tauri's CLI.

Frontend

```
pnpm i
pnpm dev
```

Backend

```
cd src-tauri
cargo run
```

1. Click "add note", there should be a `SqliteError { code: 5, message: "database is locked" }` is locked error because the database was just initialized and ran the migrations.
2. Restart the app by closing it, or pressing ctrl+c in the backend terminal. Run it with `cargo run` again.
3. Click "add note", it should succeed.
4. Click "add note" again, it should fail.

- If you click "search test notes", there should be no error, but if you click "add note", any time again during the lifetime of the app, it always errors with the same "database is locked" message.

- The db file is in `/Users/<foo>/Library/Application Support/com.tauri-app-sqlite-vec`

- Comment out lines 245-263 in src-tauri/src/main.rs and there should be no errors when inserting.
