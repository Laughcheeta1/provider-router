# Source and dependency notices

Provider Router's application source is licensed under the MIT license in [LICENSE](LICENSE).

The Codex credential interoperability contract was checked against the Apache-2.0-licensed [OpenAI Codex source](https://github.com/openai/codex/tree/1099fe069438099b8c1d6244f797ffd8515b6de0/codex-rs/login/src), particularly `token_data.rs`, `auth/storage.rs`, `auth/storage_commit.rs`, and `auth/manager.rs`. Provider Router contains an independently written TypeScript implementation of the public credential format and OAuth exchange. It does not bundle Codex or import another application's private packages.

Native locking is supplied by [fs-native-extensions](https://github.com/holepunchto/fs-native-extensions), licensed Apache-2.0. Native keyring access is supplied by [@napi-rs/keyring](https://github.com/Brooooooklyn/keyring-node); its platform packages include their own third-party notices. Electron, Electron Builder, Electron Updater, Zod, AJV and other packaged dependencies retain their distributed license notices in the installed dependency tree. The OpenAI SDK is used for development verification and is not bundled into application code.
