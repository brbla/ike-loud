# English – ike loud texts.
#
# Fluent format: https://projectfluent.org/
# New language: copy the folder i18n/en to i18n/<language code> (e.g. i18n/de)
# and translate the text to the right of “=”. Do not change the keys on the left.

## Application

app-name = ike loud
app-description = Unofficial client for iCloud Drive

## Sizes

size-bytes = { $value } B
size-kb = { $value } kB
size-mb = { $value } MB
size-gb = { $value } GB
size-tb = { $value } TB
# Character between the integer and the decimal part of a number.
decimal-separator = .

## Sync is stopped

rejected-local-folder-unavailable =
    The iCloud folder is not available right now, so syncing has stopped. Your files on iCloud are safe. If the folder is on an external drive, connect it.
rejected-suspicious-remote-listing =
    iCloud returned an empty file list although there should be files. Syncing has stopped and nothing is deleted. It will try again shortly.
rejected-conflict =
    { $count ->
        [one] One file was also changed on another device in the meantime.
       *[other] { $count } files were also changed on another device in the meantime.
    } Nothing was overwritten; syncing will sort it out shortly and keep both versions.
rejected-mass-change =
    { $changes ->
        [0] {""}
       *[other] Syncing would delete or overwrite { $changes ->
            [one] one file
           *[other] { $changes } files
        } out of { $total } within a day.{" "}
    }{ $moves ->
        [0] {""}
       *[other] Syncing would move { $moves ->
            [one] one file
           *[other] { $moves } files
        } out of { $total } within a day.{" "}
    }Please check the list of files and confirm before it continues.
rejected-icloud-space =
    There is not enough space on iCloud: { $needed } needed, { $free } free. Syncing has stopped and nothing is deleted. Free up space on iCloud or buy more.
rejected-disk-space =
    There is not enough space on this computer: { $needed } needed, { $free } free. Syncing has stopped and nothing is deleted. Free up disk space.

## A file is waiting

deferred-read-only = Only downloading for now. { $reason }
deferred-still-being-written = The file is still being saved. It will be uploaded once it is done.
deferred-ignored = Temporary program file; it is not uploaded to iCloud.
deferred-case-collision =
    iCloud already has “{ $name }”. iCloud does not distinguish upper and lower case, so please rename the file.
deferred-name-char = The name contains a character iCloud does not accept. Please rename the file.
deferred-name-too-long = The name is too long for iCloud. Please rename the file.

## Why it is only downloading

read-only-settings = Writing to iCloud is turned off in settings.
read-only-first-download = Writing to iCloud turns on once the first download has finished and been checked.
read-only-unknown-api = iCloud answered unexpectedly. To be safe, nothing is written to it until a fixed version is out.
read-only-kill-switch = Apple changed iCloud. To be safe, nothing is written to it until a fixed version is out.

## File names

# Name of a conflict copy. $stem is the name without extension, $extension
# includes the dot (may be empty), $date looks like 2026-11-15, $number
# is 1 for the first copy that day. Do not use the slash “/”.
conflict-copy-name =
    { $stem } ({ $number ->
        [1] Linux conflict { $date }
       *[other] Linux conflict { $date } { $number }
    }){ $extension }

## App window and setup (ikeloud-gui)

gui-signin-title = Sign in to iCloud
gui-signin-description = Use your Apple Account. ike loud keeps no password: it remembers the sign-in, not the password.
gui-account = Apple Account (email)
gui-password = Password
gui-signin = Sign In
gui-code-title = Verification code
gui-code-description = Apple sent a six-digit code to the phone number ending { $phone }.
gui-code = Code
gui-verify = Verify
gui-resend = Send a new code
gui-code-sent = A new code is on its way.
gui-folder-title = Where to keep your files
gui-folder-description = iCloud Drive will be downloaded into this folder. It must be new or empty: ike loud never mixes your iCloud with files that are already there.
gui-folder = Folder
gui-folder-choose = Choose…
gui-folder-done = Start downloading
gui-folder-not-empty = The folder is not empty. Choose an empty or a new one.
gui-folder-not-a-folder = This is a file, not a folder.
gui-folder-overlaps = This folder belongs to ike loud itself. Choose another one.
gui-folder-error = The folder cannot be used.
gui-error-credentials = Wrong Apple Account or password.
gui-error-code = Wrong or expired code.
gui-error-too-many = Too many attempts. Wait a while and try again.
gui-error-no-phone = The Apple Account has no trusted phone number for the code.
gui-error-offline = No connection to iCloud. Check the internet connection.
gui-error-drive-not-set-up = iCloud Drive is not set up yet. Open it once at icloud.com, then sign in again.
gui-error-drive-unavailable = This Apple Account has no iCloud Drive.
gui-error-other = Sign-in failed: { $detail }
gui-open-folder = Open Folder
gui-pause = Pause
gui-resume = Resume
gui-sign-out = Sign Out
gui-show = Show ike loud
gui-quit = Quit
gui-state-starting = Starting
gui-state-idle = Up to date
gui-state-syncing = Syncing
gui-state-needs-confirmation = Waiting for your confirmation
gui-state-signed-out = Signed out
gui-state-offline = Offline
gui-state-error = Needs attention
gui-state-paused = Paused
gui-state-idle-detail = Files on iCloud: { $files }. Downloaded since start: { $downloaded }.
gui-state-syncing-detail = Downloaded since start: { $downloaded }.
gui-state-signed-out-detail = The sign-in expired. Sign in again to continue.
gui-state-offline-detail = It will try again on its own.
gui-state-paused-detail = Nothing is downloaded until you resume.
gui-settings = Settings
gui-folder-change = Change…
gui-folder-change-description = Where iCloud Drive is downloaded. A new folder must be new or empty; the old one keeps its files.
gui-folder-changed = The folder changed. iCloud Drive is being downloaded into it.
gui-account-section = Account
gui-read-only = Read-only for now: nothing is ever written to iCloud.
