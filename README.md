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
| `rustcloud-client-<версия>-1-x86_64.pkg.tar.zst` | Arch Linux: `sudo pacman -U ./rustcloud-client-*.pkg.tar.zst` |
| `PKGBUILD` | Arch Linux: собрать пакет самому, `makepkg -si` (скачает `.deb` этого релиза) |
| `RustCloud_<версия>_amd64.AppImage` | любой Linux: всё своё внутри, `chmod +x` и запустить |
| `rustcloud-client-<версия>-linux-x86_64.tar.gz` | только исполняемый файл, без установки |
| `RustCloud_<версия>_x64-setup.exe` | Windows 10/11: установщик |
| `RustCloud_<версия>_x64_en-US.msi` | Windows: установщик для развёртывания (MSI) |
| `rustcloud-client-<версия>-windows-x86_64.zip` | Windows: только `rustcloud-client.exe`, без установки |
| `RustCloud_<версия>_android.apk` | Android 7+ (arm64, arm, x86_64) |

Исполняемому файлу из архива нужны WebKitGTK и GTK 3 из системы
(Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0`; в Fedora — `webkit2gtk4.1`;
в Arch — `webkit2gtk-4.1`).
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
app/src-tauri/plugins/account/   аккаунт телефона: аутентификатор Android (Kotlin)
tools/account-probe/             проверочное Android-приложение для аккаунта
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

### В фоне (Linux, Windows)

Приложение живёт в трее: крестик прячет окно, синхронизация продолжается. Значок показывает
состояние точкой: зелёная — всё синхронизировано, синяя — идёт синхронизация, красная — ошибка
(подробности в окне), серая — пауза. Меню трея: открыть окно, «Синхронизировать сейчас»,
«Приостановить синхронизацию» (до возобновления или перезапуска), «Запускать при входе в систему»
(приложение стартует сразу в трей), «Выход» — только он завершает приложение. Повторный запуск
открывает окно уже работающей копии.

Синхронизация начинается при старте, не дожидаясь окна; если сети ещё нет — пробует раз в минуту.
В Linux трею нужна библиотека AppIndicator (`libayatana-appindicator3-1`, пакет .deb ставит её
сам); в GNOME, кроме того, — расширение «AppIndicator and KStatusNotifierItem Support» (в Ubuntu
оно уже включено); без расширения значка не видно, но окно открывается повторным запуском
приложения. Если же библиотеки нет, трея нет вовсе, и закрытие окна завершает приложение.

## Вход

Приложение открывает системный браузер на странице сервера, пользователь входит любым способом
(пароль, OIDC) и разрешает устройство. Браузер возвращается на `http://127.0.0.1:<порт>/callback`,
приложение получает пару токенов. Refresh-токен меняется при каждом обновлении и сразу
сохраняется — старый сервер считает украденным. Подробно — `docs/api/auth.md` сервера.
Нужен сервер с коммитом «Client apps log in through the browser» или новее.

### Аккаунт телефона (Android)

После входа в настройках Android появляется «Аккаунты → RustCloud — alice@cloud.example.com».
Через него другие приложения получают доступ к облаку без своего входа, как приложения Google
через аккаунт Google. Удалить аккаунт в настройках — то же, что «Выйти» в приложении.

- Сессия у клиента одна. Другие приложения получают только access-токен на 10 минут, refresh-токен
  из клиента не уходит. Обновляет токены только клиент (Rust-ядро, `src/account.rs`): refresh-токен
  меняется при каждом обновлении, и второе приложение, которое обновило бы его само, завершило бы
  сессию.
- Приложения, подписанные тем же ключом, что и клиент, получают токен без вопросов. Приложению
  другого разработчика телефон показывает предупреждение: «Приложение X просит доступ к аккаунту
  RustCloud…». Разрешения видны на вкладке «Доступ», там их можно отозвать.
- Устроено как аутентификатор Android `AccountManager`, код — `app/src-tauri/plugins/account`.

Как приложение получает токен:

```kotlin
val am = AccountManager.get(context)
val account = am.getAccountsByType("com.rustcloud.account").firstOrNull()
    // приложение другого разработчика не видит аккаунт, пока пользователь его не выберет:
    // AccountManager.newChooseAccountIntent(null, null, arrayOf("com.rustcloud.account"), …)
am.getAuthToken(account, "api", null, activity, { future ->
    val result = future.result
    val token = result.getString(AccountManager.KEY_AUTHTOKEN)  // Authorization: Bearer …
    val server = result.getString("server")                     // https://cloud.example.com/
    val expiresAt = result.getLong("expiresAt")                 // мс с 1970; потом просить снова
}, null)
```

В манифест приложения нужно добавить фильтр, иначе с Android 11 клиент не видит приложение и не
может проверить его подпись:

```xml
<activity android:name=".MainActivity" android:exported="true">
  <intent-filter>
    <action android:name="com.rustcloud.action.USE_ACCOUNT" />
    <category android:name="android.intent.category.DEFAULT" />
  </intent-filter>
</activity>
```

Если клиент не залогинен или сессия кончилась, ответ содержит `KEY_INTENT` на экран входа клиента
(`AccountManager` открывает его сам, если передан `activity`). Если нет сети,
ответ — `ERROR_CODE_NETWORK_ERROR`. Проверочное приложение — `tools/account-probe`: CI собирает его
при запусках без тега (артефакт `android-account-probe`) в двух подписях: `ours` (ключом клиента)
и `other` (чужим).

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
с `SHA256SUMS`: Linux (`.deb`, `.rpm`, Arch `.pkg.tar.zst` и `PKGBUILD`, `.AppImage`, `.tar.gz`), Windows (`.exe`, `.msi`, `.zip`),
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
