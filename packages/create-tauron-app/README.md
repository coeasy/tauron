# create-tauron-app

Create a Tauri 2 client project with Tauron pre-wired.

```sh
npm create tauron-app@1.1.0 -- ./my-app
cd my-app
npm install
npm run tauri dev
```

To use a local Tauron checkout while developing Tauron itself:

```sh
npm create tauron-app@1.1.0 -- ./my-app --tauron-path ../tauron
```

Generated projects pin the Tauron JavaScript and Rust packages to the matching
release version. The scaffold supports Tauri 2 with React, Vue, Svelte, or
vanilla TypeScript frontends. Other host runtimes need a compatible transport
adapter; this initializer does not install native integrations for them.
