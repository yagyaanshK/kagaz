# OneTouch doorbells

The Doorbell pane supports two explicitly selected services:

- **OneTouch Smart Cloud**: migrated accounts.
- **OTIPVDP (legacy)**: the older account protocol. Its optional device MAC field
  mirrors the old app's advanced server discovery. Empty uses that APK's default.

Sign in using the account username for the selected phone app. This may differ
from the contact email address; see the vendor’s
[login guide](https://knowledge.akuvox.com/docs/smartplus-user-app-login).
Kagaz loads devices, offers
live monitoring where the account permits it, and lists cloud snapshots/videos.
A snapshot has a **View snapshot** button; only entries containing a video URL
have **Play**. Some Smart Cloud accounts expose snapshots only.
Recordings stored only on an indoor monitor are not retrieved by these cloud APIs.
Local direct-IP SIP calls remain independent of account sign-in.

## Protocol evidence

Implemented from the user-supplied APKs in `installations/doorbell-analysis`,
inspected on 2026-10-13; APKs, account data and device captures are not committed.
The relevant classes in both APKs are `LoginModel`, `UrlDefine`, `UrlTools`
(`URLTools` in the legacy app), `ApiDataModel`, and `AkcsStringRequset`.

Smart Cloud:

- `EncryptTools` and `CheckAPITools`: region `06` padded to 12 characters and
  interleaved with a random key for HTTPS `/v3.2/redirect`, API version `6.54`.
  The public discovery service was checked and returned the OneTouch gateway.
- `IntercomRemoteRepository.getOemUserAgent` / `OemDefined.DISCREET`: every
  Smart Cloud request sends `User-Agent: Onetouchsmartcloud`. The gateway uses
  this OEM identifier; a generic User-Agent caused `result: 1` even with valid
  credentials. The legacy provider retains its original client behavior.
- Current login responses use `err_code: "0"` and `datas` without `result`; RPS
  uses `result: 0`. Both success envelopes are supported, while missing or
  conflicting success codes fail closed.
- `LoginModel`: username Caesar shift by 3, password MD5 of its lowercase MD5
  hexadecimal representation; `/login`, API version `7.11`. The email `@` stays
  literal in the query, matching the Android request. Login errors distinguish
  discovery, authentication and device loading without exposing response bodies.
- `IntercomRemoteRepository`: `/userconf`, API `7.13`; `/refresh_token` JSON POST,
  API `6.8`, using the saved refresh token. One retry after token expiration.
- `UserConfigBean`, `AppConfigBean`, `AKDeviceBean`: `datas.app_conf`,
  `datas.dev_list`, `uid`, `role`, `is_need_monitor`, `mac`, and `rtsp_pwd`.
- The vendor's [Activities page](https://onetouchsmartcloud.com/smartplus/Activities.html)
  and its JavaScript: `/web-server/v4/smartplus/{single|community}/log/{master|member}/getCaptureLog`,
  `page`/`row`, `data.list`, `videoPlayUrl`, `picUrl`, `captureTime`, `deviceName`.
  It returns a look-ahead item for pagination. Backend host comes from login,
  with the OneTouch `setEnv.js` backend as fallback.

OTIPVDP:

- HTTP `/redirectv6?mac=…`, API `3.0`; `/login` with the original username and
  double-MD5 password, API `4.6`. No automatic downgrade from Smart Cloud.
- `AESUtil`: JSON or base64 AES-256-CBC login responses; first 16 characters of
  username MD5 plus the APK's fixed suffix; the APK's fixed IV and PKCS7 padding.
- HTTPS `/userconf`, API `3.0`; `vrtsp_server` with `access_server` fallback.
- The vendor's [legacy page](https://otipvdp.com/VBell/index.html) and JavaScript:
  `/apache-v3.0/getnewpersonalcaptrue` (`data.row`, `PicUrl`, `Time`), and
  `/apache-v3.0/getpersonalvideo` (`data`, `VideoUri`, `VideoTime2`). These lists
  are paginated locally. Snapshot history still displays if the video list fails,
  with an explicit notice. Expired legacy sessions require signing in again.

Video:

- APK `MonitorModel` and RTSP library: `rtsp://relay/MAC`, username `user`,
  per-device RTSP password, `Account` and `Manual: 1` on DESCRIBE.
- A random-path loopback RTSP adapter rewrites request/SDP URLs and Digest
  authorization for the upstream URI, adds those headers, and forwards TCP
  interleaved RTP. Real RTSP passwords stay in the adapter, not the engine URL.
- The existing go2rtc engine remuxes live video for the webview. Each engine now
  writes its own log so the doorbell engine cannot truncate a camera engine log.
- Selected HTTP(S) cloud media URLs go directly to the video/image element.
  Account tokens are never forwarded to media hosts. Refresh history if signed
  media URLs expire.

## Session and lifecycle

`<config>/onetouch-session.json` stores the selected service, username and tokens,
never the account password. Writes are atomic, with mode `0600` on Unix. One
account is active at a time. Logout removes the saved session and clears devices,
history and streams. Leaving the Doorbell pane or closing video stops its stream.
Native commands serialize changes so late requests cannot restore a logged-out
session. Frontend playback generations discard late video responses.

Smart Cloud gateway/API addresses must be HTTPS in the OneTouch/Akuvox domains;
legacy discovery retains the old app's HTTP gateway and IP-address support.
Neither path disables certificate verification or automatically follows redirects
containing credentials. Network errors omit request URLs and response bodies.

## Validation and limits

- Protocol tests cover login encoding, both legacy response formats, account and
  history parsing, credential redaction, private session storage and expiration.
- RTSP tests cover URL/Digest rewriting, SDP lengths, interleaved framing, and
  a real loopback connection with header injection and shutdown on drop.
- Desktop build and JavaScript syntax checks pass. A Chromium UI check with
  simulated Tauri responses exercises login failure/retry, both providers,
  unavailable monitoring, snapshot/video actions, pagination and logout.
- Desktop sign-in and device discovery have been verified against the service.
- Live video currently opens a blank player; relay troubleshooting remains open.
- The cloud history request succeeds but returns an empty list. Snapshot
  rendering with real account data remains unverified.
- A subsequent saved session returned `1000000001` (token invalid), and its
  refresh token was rejected. This code now follows the expiration path.
- A stalled browser `play()` promise no longer blocks navigation or retry controls;
  the UI check covers an indefinitely pending playback promise.
- Smart Cloud two-factor accounts and non-resident account roles are reported as
  unsupported. Local indoor-monitor recordings, two-way cloud audio, cloud call answering,
  and cloud door release are not implemented by this account integration.

### Authentication verification

The supplied app (`7.31.2205`) uses the End user endpoint and OEM User-Agent.
A desktop authentication check failed with a generic User-Agent and succeeded
when the OEM header was added. No raw credentials, token values or account logs
are committed.

Signing in from another client may replace an existing session. Simultaneous
phone/desktop use remains unverified. Do not automatically retry passwords or
switch login providers.
