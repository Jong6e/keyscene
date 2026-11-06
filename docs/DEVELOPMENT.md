# Keyscene 개발 문서

사용 설명은 [README](../README.md)에 있습니다. 이 문서는 고치고 빌드하는 사람을 위한 내용입니다.
Rust + [Tauri 2](https://tauri.app) 앱이고, 화면(`ui/`)은 빌드 도구 없는 HTML/CSS/JS입니다.

## 빌드

```powershell
cargo test --release                              # 단위 테스트
cargo test --release -- --ignored --nocapture     # 실제 PC에서: 모니터·앱 목록 출력, 메모장 배치·원래대로 시험
pwsh -File tools/build-setup.ps1                  # 설치 파일: target/release/bundle/nsis/Keyscene_<버전>_x64-setup.exe
```

- `tauri` 명령은 `cargo install tauri-cli --version "^2" --locked`(또는 `npm i -g @tauri-apps/cli@2`)로 설치합니다. 첫 빌드는 NSIS를 내려받습니다.
- `tools/build-setup.ps1`은 `tools/build-env.ps1`로 사용자 폴더 경로를 `~`로 바꿔 빌드합니다. 그냥 빌드하면 exe 안에 `C:\Users\<이름>\.cargo\...` 같은 경로가 남습니다.
- 설치는 현재 사용자용(`%LOCALAPPDATA%\Keyscene`, 관리자 권한 없음)입니다. 설치 파일은 WebView2가 없으면 Microsoft에서 받아 설치하고, 제거할 때 시작 프로그램 항목을 지웁니다.
- 아이콘은 `python -I tools/make_icons.py`로 다시 만듭니다. 트레이 아이콘은 16·20·24·32px을 만들어 화면 배율에 맞는 것을 씁니다.

## 릴리스

1. `tauri.conf.json`과 `Cargo.toml`의 `version`을 함께 올리고, `release-notes/v<버전>.md`를 씁니다.
2. 태그 `v<버전>`을 올리면 GitHub Actions(`.github/workflows/release.yml`)가 깨끗한 Windows에서 테스트·빌드하고, 설치 파일과 SHA-256을 **초안(draft) 릴리스**로 올립니다.
3. 초안의 파일을 내려받아 설치해 본 뒤 공개(Publish)합니다.

## 동작 방식

- **단축키** 하나 = 이름 + 키 조합 + 동작 목록(코드에서는 `preset`). 처음 실행하면 추천 단축키 3개를 넣고 창을 한 번 엽니다.
- 실행하면 동작마다 원래대로 할 정보(`engine::Undo`)를 모읍니다. 같은 키를 다시 누르면 **역순으로, 그 뒤 사용자가 직접 바꾸지 않은 것만** 원래대로 합니다. 원래대로 할 것이 하나도 남지 않았으면 그 누름은 다시 실행합니다.
- **유지 시간**: 단축키마다 N분이 지나면 원래대로. 기다리는 동안만 1분에 한 번 깨어 트레이 메뉴의 남은 시간을 고칩니다.
- **전환** 동작(숨긴 항목·바탕 화면 아이콘·마이크·스피커·어둡게/밝게): 실행할 때의 상태를 뒤집고, 다시 누르면 실행 전 상태로 돌립니다.
- 앱 열기에서 원래 열려 있던 창은 '닫기'로 정해 두어도 원래대로 할 때 닫지 않고 최소화만 합니다.
- Keyscene이 실행 중에 꺼져도 설정류(음소거·아이콘·숨긴 항목·테마·음량)의 원래대로 할 정보는 `restore.json`에 남아 다음에 원래대로 할 수 있습니다.
- **하위 항목까지 숨기기**: 디렉터리 목록에 이미 들어 있는 속성을 보고 바뀌어야 하는 항목에만 씁니다(파일 2만 개 중 새 파일 10개: 전체 다시 쓰기 2.9초 → 0.1초). 바로 가기·정션 안으로 들어가지 않고, 드라이브 전체·Windows·Program Files·ProgramData·사용자 폴더 전체는 실제 경로로 비교해 거절합니다. 10분 동안 트레이에서 실행 취소할 수 있습니다.
- **평소 비용 0**: 폴링 없이 Windows 알림만 씁니다(오디오 장치·음소거 변경, 작업 표시줄 테마, 맨 앞 창 바뀜). 대기 중 CPU 0, 메모리 약 16MB(고유 3MB).
- **창**: 시작할 때 만들지 않고, 열 때 만들고 닫으면 없앱니다(WebView2가 열려 있는 동안 약 150MB, 닫으면 반환). 위치·크기를 기억하고 화면 가장자리 12px(배율 비례) 안에서 붙습니다.
- **단축키를 쓰지 않을 앱**: 그 프로그램이 맨 앞에 있는 동안 등록한 키를 모두 놓습니다.
- **명령줄**: `keyscene.exe --run "<이름 또는 id>"`. 실행 중인 Keyscene에 전달되고, 꺼져 있으면 켜면서 실행합니다.
- **네트워크**: "새 버전 확인"을 누를 때만 `api.github.com`에 접속합니다(CSP `connect-src`로 그 주소만 허용).

## 구조

| 파일 | 하는 일 |
| --- | --- |
| `src/main.rs` | 트레이, 메뉴, 키 등록, 단축키 실행·원래대로·유지 시간, 창 명령 |
| `src/preset.rs` | 단축키·동작 데이터 구조(설정 파일과 화면이 같이 씀) |
| `src/engine.rs` | 동작 실행과 원래대로 |
| `src/config.rs` | 설정 파일 읽기/저장(임시 파일에 쓴 뒤 교체, 손상되거나 읽지 못한 파일은 보관) |
| `src/explorer.rs` | 숨긴 항목·바탕 화면 아이콘, 열린 탐색기 창 새로 고침 |
| `src/audio.rs` | 마이크 음소거와 변경 알림 |
| `src/launch.rs` | 앱·파일·웹 열기, 창 찾기·배치, 모니터·설치된 앱·브라우저 프로필 목록 |
| `src/system.rs` | 스피커, 테마, 전원, 클립보드, 키 입력, 창 조작 |
| `src/shell.rs` | 탐색기에서 선택한 항목 읽기, 숨김 속성 바꾸기 |
| `src/tree.rs` | 폴더 안 전체 숨기기/숨김 해제와 실행 취소 |
| `src/focus.rs` | 맨 앞 창이 바뀔 때 알림 |
| `src/icons.rs` | 창에 보이는 앱·파일 아이콘(PNG) |
| `src/winplace.rs` | 창 위치 기억, 화면 가장자리에 붙기 |
| `src/dialog.rs`, `src/reg.rs`, `src/errlog.rs` | 파일 선택 창, 레지스트리, 오류 기록 |
| `ui/` | 창 화면. `components.js` 공용 부품, `presets.js` 단축키 목록·편집, `excluded.js` 단축키를 쓰지 않을 앱 |
| `tools/` | `build-setup.ps1` 설치 파일 빌드, `make_icons.py` 아이콘 생성 |

## 데이터

- 설정: `%APPDATA%\Keyscene\config.json`(형식은 `src/config.rs`, `src/preset.rs`). 원래대로 할 정보: `restore.json`.
- 오류 기록: `keyscene.log`(실패했을 때만 한 줄씩, 256KB를 넘으면 `keyscene.old.log`로 넘김). 예상하지 못한 중단(panic)도 남습니다.
- 시험할 때는 `APPDATA` 환경 변수를 빈 폴더로 바꿔 실행하면 그 아래 `Keyscene` 폴더를 씁니다. 첫 실행·시연 화면을 실제 설정과 섞지 않고 볼 수 있습니다.
