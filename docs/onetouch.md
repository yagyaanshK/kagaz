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
  Network failures and temporary HTTP failures try the APK's HTTPS backup
  `bk.rps.akuvox.com/v3.2/redirect`, with API `3.0` as used by
  `IntercomRemoteRepository.requestBKGetRedirect`. Account credentials are not
  sent to either discovery endpoint; returned gateway origins are validated.
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
  authorization for the upstream URI. It follows the relay's session-specific
  `Content-Base` over the existing authenticated connection, even when that base
  differs from the original hostname and device path.
- The relay returns UDP transport even when TCP is requested. The adapter maps
  the engine's loopback RTP/RTCP ports to relay sockets, includes `X-SSRC`, and
  sends a NAT-opening packet from the advertised port. Packets are forwarded
  only between the negotiated engine and relay endpoints. Closing playback stops
  these sockets. RTP obfuscation is not supported.
- go2rtc source options use `#transport=udp#backchannel=0` (separate `#` options,
  as parsed by its [RTSP handler](https://github.com/AlexxIT/go2rtc/blob/v1.9.14/internal/rtsp/rtsp.go)).
  Real RTSP passwords stay in the adapter, not the engine URL.
- Smart Cloud SDP can advertise H.264 parameter sets that differ from the live
  RTP's SPS/PPS. Reinjecting those stale sets at keyframes caused native decoder
  errors and freezes. Smart Cloud uses the configuration carried in the stream;
  legacy OTIPVDP retains the SDP configuration. Audio and other codecs are unchanged.
- Smart Cloud H.264 UDP delivery uses an 80 ms reorder window (at most 128
  queued packets) and bounded frame assembly. Missing fragments discard the
  incomplete picture and dependent pictures until a complete keyframe arrives.
  The active in-band SPS/PPS precede each forwarded keyframe. This keeps damaged
  pictures out of the native decoder; playback can briefly wait for a keyframe
  when packets are lost.
- Live playback uses the pinned engine's `VideoRTC` MSE player over a loopback
  WebSocket. The raw MPEG-TS route decoded in a standalone GStreamer pipeline
  but failed in WebKit's video element; fragmented MP4 through MSE plays there.
  Recorded media retains its ordinary video/image element. Closing live playback
  disconnects the player and releases its timers as well as the RTSP bridge.
- After UDP setup, go2rtc manages control-connection liveness using the negotiated
  RTSP keepalive interval. The bridge's initial 30-second handshake deadline no
  longer interrupts an otherwise active UDP stream.
- Each engine writes its own log so the doorbell engine cannot truncate a camera
  engine log.
- Selected HTTP(S) cloud media URLs go directly to the video/image element.
  Account tokens are never forwarded to media hosts. Refresh history if signed
  media URLs expire.

## Session and lifecycle

`<config>/onetouch-session.json` stores the selected service, username and tokens,
never the account password. Writes are atomic, with mode `0600` on Unix. One
account is active at a time. **Remember sign-in securely** stores the successful
login separately in the OS credential store: Secret Service on Linux, Keychain
on macOS, Credential Manager on Windows. The implementation uses
[keyring](https://docs.rs/keyring/3.6.3/keyring/). No plaintext-file fallback is used;
if the vault is unavailable, sign-in can succeed with a notice that remembering
failed. This feature is not yet offered on mobile builds.

Expiration and startup do not replay passwords. **Reconnect** makes one login
attempt only when clicked, using the saved provider and legacy discovery input.
The webview receives remembered-account metadata, never the saved password.
**Sign out** preserves the remembered login; **Forget saved sign-in** removes it
from the vault without ending an active session.

Logout removes the saved token session and clears devices,
history and streams. Leaving the Doorbell pane or closing video stops its stream.
Native commands serialize changes so late requests cannot restore a logged-out
session. Frontend playback generations discard late video responses.

Smart Cloud gateway/API addresses must be HTTPS in the OneTouch/Akuvox domains;
legacy discovery retains the old app's HTTP gateway and IP-address support.
Neither path disables certificate verification or automatically follows redirects
containing credentials. Network errors omit request URLs and response bodies.

## Validation and limits

- Credential-store tests cover invalid records and metadata-only UI responses.
  A synthetic credential was stored, read, and deleted in the Linux OS vault.
  UI checks cover manual reconnect after expiry, no automatic password replay,
  retry after failure, legacy account remembering, and forgetting credentials.
- Protocol tests cover discovery fallback and gateway validation, login encoding,
  both legacy response formats, account and
  history parsing, credential redaction, private session storage and expiration.
- RTSP tests cover URL/Digest rewriting, session-specific SDP bases, framing,
  a real control connection, UDP video/feedback forwarding, unrelated-sender
  rejection, shutdown on drop, and H.264 SDP cleanup without changing other codecs.
  RTP tests cover reordering, duplicates, sequence wrap, malformed packets,
  fragment loss and recovery at the next complete keyframe.
- Desktop build and JavaScript syntax checks pass. A Chromium UI check with
  simulated Tauri responses exercises login failure/retry, both providers,
  unavailable monitoring, snapshot/video actions, pagination and logout.
- Desktop sign-in and device discovery have been verified against the service.
- The initial standalone/MSE checks did not establish stable playback in Kagaz.
  Native playback diagnostics reproduced decoder failures with stale SDP and
  lost/out-of-order UDP fragments. After the fixes, a check in the actual Kagaz
  window ran for over three minutes and delivered over 4,000 rendered frames,
  with no media-error event or player restart. Hardware-decoder allocation
  warnings occurred without a fatal playback error. The user confirmed continuous
  playback in Kagaz on 2026-10-14.
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
