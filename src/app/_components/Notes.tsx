"use client";

import { invoke } from "@tauri-apps/api/tauri";

export default function Notes() {
  return (
    <div className="flex flex-col gap-2">
      <div>NOTES</div>
      <button
        onClick={() => {
          invoke("search_notes", { query: "test" })
            .then((res) => {
              console.log("search notes res: ", res);
            })
            .catch(console.error);
        }}
      >
        Search test notes
      </button>

      <button
        className="bg-red-200"
        onClick={() => {
          invoke("add_note", { content: "test" })
            .then((result) => {
              console.log("add_note result", result);
            })
            .catch(console.error);
        }}
      >
        Add note
      </button>
    </div>
  );
}
