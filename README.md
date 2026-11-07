# RustCloud Client

Клиент для [RustCloud](https://github.com/RustCloud/server): просмотр файлов на сервере,
синхронизация выбранных папок и (позже) WASM-плагины прямо на устройстве.
Одна кодовая база для Linux, Windows и Android.

**Статус: ранняя версия.** Есть вход через браузер, просмотр папок и двусторонняя синхронизация
выбранных папок. Дальше — плагины на устройстве, Windows и Android.

## Установка

Готовые сборки — на странице [Releases](https://github.com/RustCloud/client/releases):

| Файл | Для чего |
|------|----------|
| `RustCloud_<версия>_amd64.deb` | Debian, Ubuntu: `sudo apt install ./RustCloud_*.deb` |
| `RustCloud-<версия>-1.x86_64.rpm` | Fedora, openSUSE: `sudo dnf install ./RustCloud-*.rpm` |
| `RustCloud_<версия>_amd64.AppImage` | любой Linux: всё своё внутри, `chmod +x` и запустить |
| `rustcloud-client-<версия>-linux-x86_64.tar.gz` | только исполняемый файл, без установки |
| `RustCloud_<версия>_x64-setup.exe` | Windows 10/11: установщик |
| `RustCloud_<версия>_x64_en-US.msi` | Windows: установщик для развёртывания (MSI) |
| `rustcloud-client-<версия>-windows-x86_64.zip` | Windows: только `rustcloud-client.exe`, без установки |
| `RustCloud_<версия>_android.apk` | Android 7+ (arm64, arm, x86_64) |

Исполняемому файлу из архива нужны WebKitGTK и GTK 3 из системы
(Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0`; в Fedora — `webkit2gtk4.1`).
Windows-сборкам нужен WebView2 — в Windows 10/11 он уже есть, установщик при необходимости
поставит его сам. На Android синхронизируемые папки лежат в общей памяти устройства: дайте
приложению доступ — Настройки → Приложения → RustCloud → Разрешения → Файлы и медиа →
«Разрешить управление всеми файлами».

Проверить загрузку: `sha256sum -c SHA256SUMS --ignore-missing`.

## Устройство

```text
crates/core/      rc-core: вход (OAuth 2.0 + PKCE), API сервера, лента изменений
crates/sync/      rc-sync: двусторонняя синхронизация папки (состояние в SQLite)
app/              интерфейс на Svelte 5
app/src-tauri/    приложение Tauri 2: команды поверх rc-core и rc-sync
```

Ядро и синхронизация не знают об интерфейсе: их можно использовать и из консольного клиента.

## Синхронизация

Во вкладке «Синхронизация» две панели: слева папки компьютера, справа — облака. Щелчок выбирает
папку (в том числе ту, что открыта, или всё облако), двойной щелчок или `›` открывает; в каждой
панели можно создать новую папку. Выбрали обе — «Синхронизировать», и пара появится в списке ниже.
Синхронизируются только выбранные пары. Дальше папки держатся одинаковыми: по подсказкам сервера
(лента изменений), по изменениям на диске и раз в 5 минут.

- Изменение с одной стороны переносится на другую. Изменения с обеих сторон с одинаковым
  содержимым — не конфликт; с разным — локальная версия сохраняется рядом как
  `имя (conflict дата).ext` и тоже уходит на сервер: ничего не теряется.
- Переименование на сервере переименовывает и локально, без скачивания заново.
- Если локальной папки нет (отключён диск) или папка на сервере удалена — синхронизация
  останавливается и ничего не удаляет. Удаление больше половины всего ждёт подтверждения.
  Замена файла на сервере — только если он не изменился с прошлого раза (`If-Match`).
- Папки с точкой в начале имени (`.git`; на сервере `.trash`, `.appdata`) не синхронизируются,
  файлы с точкой (`.gitignore`) — да. Ссылки (symlink) пропускаются.
- Пока нет: распознавания перемещений на устройстве (перемещённый файл загружается заново),
  файловых систем без учёта регистра (Windows), загрузки по частям.

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
cargo test -p rc-core -p rc-sync          # без сервера: модульные тесты, синхронизация — на сервере в памяти
cd app && npm run check
```

Интеграционные тесты (`--features rc-core/testing`) идут против настоящего сервера — работающего
или запущенного для теста:

```sh
# работающий сервер и существующий аккаунт: тесты работают в своих папках в rc-client-tests/
# и удаляют их за собой
RC_TEST_URL=http://cloud.example:8080/ RC_TEST_USER=… RC_TEST_PASSWORD=… \
  cargo test -p rc-core -p rc-sync --features rc-core/testing
# свежий сервер из бинаря (так в CI)
RC_TEST_SERVER=/path/to/rustcloud cargo test -p rc-core -p rc-sync --features rc-core/testing
```

## Релизы

Тег `vX.Y.Z` (совпадает с `version` в [Cargo.toml](Cargo.toml)) запускает
[release.yml](.github/workflows/release.yml): пакеты для всех платформ матрицы и релиз на GitHub
с `SHA256SUMS`: Linux (`.deb`, `.rpm`, `.AppImage`, `.tar.gz`), Windows (`.exe`, `.msi`, `.zip`),
Android (`.apk`). Пуш в ветку `release-check` собирает всё то же без публикации (пакеты — в
артефактах запуска). Версии с суффиксом (`-beta.1`) должны быть числовыми (`-1`): этого
требует MSI.

APK подписывается ключом из секретов репозитория `ANDROID_KEYSTORE` (base64 файла `.jks`),
`ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`. Без них — разовым ключом: APK ставится, но
не обновляет установленный другим ключом. Ключ один раз:

```sh
keytool -genkeypair -keystore rustcloud.jks -alias rustcloud -keyalg RSA -keysize 4096 -validity 10000
base64 -w0 rustcloud.jks   # → ANDROID_KEYSTORE
```
