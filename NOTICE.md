# Source and dependency notices

Provider Router's application source is licensed under the MIT license in [LICENSE](LICENSE).

The Codex credential interoperability contract was checked against the Apache-2.0-licensed [OpenAI Codex source](https://github.com/openai/codex/tree/1099fe069438099b8c1d6244f797ffd8515b6de0/codex-rs/login/src), particularly `token_data.rs`, `auth/storage.rs`, `auth/storage_commit.rs`, and `auth/manager.rs`. Provider Router uses an independently written Rust implementation of the public credential format and OAuth exchange.

[Cargo.lock](Cargo.lock) records the native executable's third-party dependencies. It uses Ratatui and Crossterm for the terminal, Axum and Reqwest for HTTP, Keyring for OS credential storage, and Hayro, Calamine, Quick XML, and CSV for document processing. Dependencies are subject to their respective licenses.
