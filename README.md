# RustCloud Client

Клиент для [RustCloud](https://github.com/RustCloud/server): просмотр файлов на сервере,
синхронизация выбранных папок и (позже) WASM-плагины прямо на устройстве.
Одна кодовая база для Linux, Windows и Android.

**Статус: начало работы.** Есть вход через браузер и просмотр папок. Дальше — синхронизация.

## Устройство

```text
crates/core/      rc-core: вход (OAuth 2.0 + PKCE), API сервера; дальше — синхронизация, плагины
app/              интерфейс на Svelte 5
app/src-tauri/    приложение Tauri 2: команды поверх rc-core
```

Ядро не знает об интерфейсе: его можно использовать и из консольного клиента.

## Вход

Приложение открывает системный браузер на странице сервера, пользователь входит любым способом
(пароль, OIDC) и разрешает устройство. Браузер возвращается на `http://127.0.0.1:<порт>/callback`,
приложение получает пару токенов. Refresh-токен меняется при каждом обновлении и сразу
сохраняется — старый сервер считает украденным. Подробно — `docs/api/auth.md` сервера.
Нужен сервер с коммитом «Client apps log in through the browser» или новее.

## Сборка

Нужны Rust (stable) и Node.js 22+; на Linux — WebKitGTK:

```sh
sudo apt install libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev file
cd app && npm ci
npm run tauri dev      # окно с перезагрузкой при изменениях
npm run tauri build    # пакеты в target/release/bundle/
```

## Тесты

```sh
cargo test --workspace
# с настоящим сервером (интеграционные тесты rc-core):
RC_TEST_SERVER=../server/target/debug/rustcloud cargo test -p rc-core
cd app && npm run check
```

## Релизы

Тег `vX.Y.Z` (совпадает с `version` в [Cargo.toml](Cargo.toml)) запускает
[release.yml](.github/workflows/release.yml): пакеты для всех платформ матрицы и релиз на GitHub
с `SHA256SUMS`. Сейчас — Linux (`.deb`, `.rpm`, `.AppImage`); Windows и Android добавятся
записями в матрицу.
