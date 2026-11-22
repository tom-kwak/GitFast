# gf Windows 출시 작업 문서

> 이 문서는 **Windows PC에서 작업을 이어받는 사람(또는 에이전트)** 을 위한 체크리스트다. 코드는 아직 수정하지 않았고, 아래는 macOS에서 코드를 읽어 정리한 분석·계획이다. Windows에서 빌드·실행해 본 적은 없으므로, 단계 0의 결과에 따라 범위를 조정한다.

## 0. 목표와 결정 사항

- **목표**: Windows x64용 첫 릴리스. macOS 동작·표시는 한 글자도 바뀌지 않는다.
- **배포 형식**: `zip` + `install.ps1` (MSI·winget은 후속)
- **서명**: 첫 릴리스는 없음 → SmartScreen 경고를 README·릴리스 노트에 안내
- **git 미설치 PC**: 시작 시 안내 화면(Git for Windows 링크 + 설정에서 `git.executable` 지정)
- **작업 장소**: 전부 Windows PC에서 직접 진행. 단계마다 그 자리에서 실행해 확인한다.
- 프로젝트 규칙(CLAUDE.md): 완료 기준 = `cargo fmt` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace`. 커밋은 사용자가 요청할 때만. 색은 테마 토큰으로만. UI 스레드에서 git·큰 파일 I/O 금지.

## 1. 현재 상태 (분석 결과)

**이미 되어 있음**
- UI: iced 0.14(wgpu) · `rfd`는 Windows 지원
- 설정·로그 경로: `directories` 사용 → `%APPDATA%` ([paths.rs](crates/gf-config/src/paths.rs)에 표 있음)
- `reveal()`에 `explorer /select,` 분기 ([outputs.rs:1489](crates/gf-app/src/app/outputs.rs))
- git은 CLI 서브프로세스 + gix. `gix-fetch`는 native-tls → Windows에서 schannel
- 단축키 동작은 `modifiers.command()` → Windows에서 Ctrl로 매핑. `Ctrl+Tab`은 이미 `shortcut()`에서 먼저 처리 ([mod.rs:2129](crates/gf-app/src/app/mod.rs))
- 일부 `cfg(unix)` 분기, 테스트 지원의 `NUL`/`/dev/null` 분기

**비어 있음 (이번 작업)**
- `windows_subsystem` 미설정 → 콘솔 창이 같이 뜸
- git 서브프로세스에 `CREATE_NO_WINDOW` 없음 → 콘솔 깜빡임
- `tool_path.rs`의 추가 PATH가 macOS(Homebrew) 경로뿐, `:` 구분자 가정 테스트
- 단축키 표시가 mac 글리프 하드코딩 (`⌘` 235회 등, ui_sweep 포함)
- "reveal in Finder" 문구
- 테스트의 Unix 가정(`cp -Rp` 등), CI는 macOS만
- Windows 패키징·릴리스 워크플로·설치 스크립트·아이콘(.ico) 없음
- README는 "gf runs on macOS today"

## 2. Windows 개발 환경 준비

- [ ] **Visual Studio Build Tools 2022**: "C++를 사용한 데스크톱 개발"(MSVC + Windows SDK). `syntect`의 onig(C 컴파일)에 필요
- [ ] **Rust**: `rustup` stable (`rust-toolchain.toml`, MSRV 1.92), 타깃 `x86_64-pc-windows-msvc`
- [ ] **Git for Windows**: 개발 중 PATH에 **있는 상태와 없는 상태** 둘 다 테스트할 수 있게 한다
- [ ] 저장소는 짧은 경로에 둔다(예: `C:\src\gf`) + `git config core.longpaths true`
- [ ] 줄바꿈: `git config core.autocrlf false`, 저장소에 `.gitattributes`(`* text=auto eol=lf`, `*.ps1 eol=crlf`)가 있는지 확인·없으면 추가. `rustfmt.toml`의 `newline_style` 확인. (안 하면 `cargo fmt --check`와 테스트 fixture가 CRLF로 어긋남)
- [ ] 셸은 PowerShell. CLAUDE.md의 `export PATH="/opt/homebrew/..."`는 macOS 전용 → Windows용 안내 한 줄을 CLAUDE.md에 추가
- [ ] 한글 IME 확인용으로 한글 입력기 활성화
- [ ] 작업 브랜치: `develop`에서 분기. push하면 CI의 macOS 잡이 macOS 회귀를 검증한다(공유 코드를 건드리므로 필수)

## 3. 단계 0 — 빌드 spike (먼저, 반나절~1일)

코드 수정 전에 `cargo build --release -p gf-app`를 Windows에서 돌리고 막히는 곳을 기록한다.

- [ ] **onig**: C 컴파일 실패 시 `syntect`/`two-face`를 `fancy-regex` 백엔드로 전환 ([Cargo.toml](Cargo.toml) workspace.dependencies)
- [ ] **winit 포크** (`[patch.crates-io]`, macOS 한글 IME용): Windows에서 빌드되는지, **Windows 한글 IME가 정상 동작하는지** 확인. patch는 플랫폼별 지정이 불가능하므로 문제가 생기면 대응책(포크 수정·분리)을 별도 검토
- [ ] **wgpu 백엔드**: DX12/Vulkan 기동, 구형 GPU·VM에서의 동작. 필요하면 tiny-skia 폴백 검토
- [ ] 첫 실행 스크린샷: 글리프(`… ↑↓←→ ▸▾ ● ✓ ✗ × ⇥ ⏎`) 렌더링, DPI, 폰트(`Font::MONOSPACE` = Consolas 계열)
- [ ] 산출: spike 결과 메모 → 이후 단계 범위 조정

## 4. 단계 1 — 코드 수정

### 4-1. 콘솔 창 제거
- [ ] [main.rs](crates/gf-app/src/main.rs) 최상단: `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`
- [ ] `CREATE_NO_WINDOW`(0x08000000) 적용
  - [runner.rs:197](crates/gf-git/src/cli/runner.rs) (`tokio::process::Command`의 `creation_flags`, `#[cfg(windows)]`)
  - [gix_fetch.rs:61](crates/gf-git/src/backend/gix_fetch.rs) (`std::os::windows::process::CommandExt`)
  - `unsafe_code = "forbid"`와 무관(안전 API)
- [ ] `reveal()`의 `explorer /select,<경로>`가 한글·공백 경로, 드라이브 문자에서 동작하는지 확인

### 4-2. git 탐색 + 미설치 안내
- [ ] [tool_path.rs](crates/gf-core/src/tool_path.rs): `cfg(windows)`용 EXTRA 추가 — `%ProgramFiles%\Git\cmd`, `%ProgramFiles%\Git\bin`, `%LOCALAPPDATA%\Programs\Git\cmd`. 테스트는 `cfg(unix)`/`cfg(windows)`로 분리(현재 `:` 구분자·macOS 경로 가정)
- [ ] 시작 시 `git --version` 확인(비동기, UI 스레드 밖; [gix_fetch.rs:61](crates/gf-git/src/backend/gix_fetch.rs)의 `git_follows_remote_head`와 같은 방식). 못 찾으면 Welcome 화면에 배너: Git for Windows 다운로드 링크 + "설정에서 git 경로 지정"(이미 있는 `git.executable`, [settings.rs:87](crates/gf-config/src/settings.rs)). 실행기의 "git executable not found" 에러([runner.rs:225](crates/gf-git/src/cli/runner.rs))와 메시지 일원화
- [ ] Windows git 동작 점검: `core.autocrlf`·CRLF가 섞인 diff 파싱, 경로 quoting(BASE_ARGS의 `core.quotepath`), 긴 경로, 한글 파일명

### 4-3. 단축키 표시 (표시만 바꾼다, 동작은 그대로)
- [ ] 신규 `crates/gf-app/src/widgets/keys.rs`: `pub fn local(s: &str) -> Cow<str>`
  - macOS: 입력 그대로 반환(무변경)
  - 그 외: `⌘→Ctrl` `⇧→Shift` `⌥→Alt` `⌃→Ctrl` `⇥→Tab` `⏎→Enter` `⌫→Backspace` `⎋→Esc`, 연속 수식 글리프는 `+`로 연결 (`⌘⇧K`→`Ctrl+Shift+K`, `⌃⇥`→`Ctrl+Tab`, `⌘1-9`→`Ctrl+1-9`)
  - 순수 함수 + 변환 표 단위 테스트
- [ ] 문자열 리터럴을 일일이 고치지 말고 **렌더 경계**에서 적용
  - `widgets::tip` / `tip_right` / `tip_at_cursor` ([widgets/mod.rs:135-181](crates/gf-app/src/widgets/mod.rs))
  - `:help` 줄 테이블 ([prompt/parse.rs:55-64](crates/gf-app/src/features/prompt/parse.rs) 등)
  - 컨텍스트 메뉴 `Entry::item` hint, 다이얼로그 버튼 라벨(`⏎`), repo_picker·tab_switcher·status_line·welcome의 도움말 줄
  - 주요 파일: `app/{mod,outputs,focus,ops,dialogs}.rs`, `features/{repo_picker,tabs,diff_view,dialog,undo_list,history,changes,status_line,tab_list,welcome,tab_switcher}/mod.rs`, `git_actions.rs`, `repo_tab.rs`
- [ ] Ctrl 충돌 점검 (Windows에서는 `command()` == `control()`)
  - `⌃`를 단독으로 쓰는 곳: 탭 스위처 해제 `!m.control()` ([mod.rs:594](crates/gf-app/src/app/mod.rs)), [mod.rs:1282](crates/gf-app/src/app/mod.rs)
  - `Ctrl+Backspace`(Forget)가 텍스트 입력의 단어 삭제와 겹치지 않는지 (`app/focus.rs` 키 수신 순서)
  - `Ctrl+H/L/J/B/E`, `Alt` 조합이 Windows 관례·메뉴 활성화와 충돌하지 않는지
- [ ] 플랫폼 문구: "reveal in Finder"([diff_view/mod.rs:1048](crates/gf-app/src/features/diff_view/mod.rs), [changes/mod.rs:253](crates/gf-app/src/features/changes/mod.rs)) → `cfg`로 "reveal in File Explorer". [ui_sweep.rs:2086](crates/gf-app/src/app/ui_sweep.rs)의 `shows("reveal in Finder")`도 같이 분기
- [ ] 비수식 글리프가 Consolas 폴백에서 누락되면 `keys.rs`에 대체 문자 매핑 추가
- [ ] [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) §4-5 "단축키는 발견 가능하게"에 규칙 한 줄 추가: 문구는 mac 글리프로 쓰고 `widgets::keys::local`을 거친다

### 4-4. 기타
- [ ] 창·작업표시줄 아이콘: `iced::window::Settings.icon`(PNG) 또는 exe 임베드 중 택일
- [ ] `gf-watch`(notify 8): `ReadDirectoryChangesW` 동작, 경로 구분자(`\` vs `/`)로 워킹트리 상대경로를 비교하는 곳 점검
- [ ] 파일시스템 경로를 문자열로 만들어 비교하는 곳은 `Path` API로. git·URL 문자열의 `'/'` 분할([git_actions.rs:71](crates/gf-app/src/git_actions.rs), [outputs.rs:948](crates/gf-app/src/app/outputs.rs))은 대체로 안전
- [ ] 설정·로그 저장 위치(`%APPDATA%\gf\gf\…`) 실기 확인. 이번 범위에서 `gf-auth`는 제외(자격증명은 git credential helper = Git Credential Manager에 위임)

## 5. 단계 2 — 테스트·CI

- [ ] [ci.yml](.github/workflows/ci.yml) 매트릭스에 `windows-latest` 추가 (주석 "Add … in M9"). `RUSTFLAGS=-D warnings`, fmt, clippy 동일
- [ ] 테스트의 Unix 가정 수정
  - [gf-test-support/src/lib.rs:117](crates/gf-test-support/src/lib.rs) `Command::new("cp").arg("-Rp")` → `std::fs` 재귀 복사 헬퍼
  - 그 외 쉘 스크립트·`/tmp`·`/dev/null`·경로 구분자·CRLF 가정은 첫 Windows 로컬 실행 결과로 수정 (`gf-git` 통합 테스트, `ops/log.rs`·`cli/parse/util.rs`의 `cfg(unix)` 테스트 쌍)
  - `ICED_TEST_BACKEND=tiny-skia`([.cargo/config.toml](.cargo/config.toml))는 Windows에서도 유효
- [ ] UI sweep(`cargo test -p gf-app -- --ignored sweep_`)은 우선 macOS 전용으로 유지, Windows는 일반 `cargo test`까지
- [ ] `cargo deny check`: Windows 전용 의존성(`windows-sys`, `clipboard-win` 등) 라이선스 확인 ([deny.toml](deny.toml))

## 6. 단계 3 — 패키징·릴리스

- [ ] `scripts/package-windows.ps1`: `cargo build --release --locked -p gf-app --target x86_64-pc-windows-msvc` → `dist/gf-windows-x64.zip`(`gf.exe`, 라이선스/README 발췌) + `gf-windows-x64.zip.sha256`. 고정 파일명 규칙 유지(`releases/latest/download/<name>`). 로컬에서 먼저 실행하고 CI도 같은 스크립트 호출
- [ ] `packaging/windows/gf.ico` ([gf.icns](packaging/macos/gf.icns)에서 변환) + `build.rs`/`winres`(또는 `embed-resource`)로 exe 임베드, 제품명·버전은 workspace 버전 사용
- [ ] `scripts/install.ps1`: 최신 릴리스 zip 다운로드 → `Unblock-File`(Mark-of-the-Web) → SHA-256 검증 → `%LOCALAPPDATA%\Programs\gf`에 설치(덮어쓰기 업데이트) → 사용자 PATH·시작 메뉴 바로가기(선택). `GF_ZIP_URL`/`GF_INSTALL_DIR` 환경변수는 `install.sh`와 대칭
- [ ] [release.yml](.github/workflows/release.yml)
  - `windows` 잡 추가 (`windows-latest`, `needs: [resolve, ci]`, 태그 체크아웃·버전 검증은 `macos` 잡과 같은 로직을 PowerShell로, `upload-artifact`)
  - `publish` 잡의 `needs`에 `windows` 추가, 두 아티팩트를 받아 `gh release create` 자산으로 함께 첨부, 릴리스 노트에 Windows 설치 안내(`irm …/install.ps1 | iex`) 추가
  - 주의: `ci` 재사용 워크플로가 Windows를 포함하게 되므로 릴리스 게이트에도 Windows 테스트가 들어감
- [ ] 문서
  - [README.md](README.md): "gf runs on macOS today" 갱신, Windows 설치 섹션, 요구사항(Git for Windows, Windows 10 1809+/11), SmartScreen 안내 + SHA-256 검증법
  - [PLAN.md](PLAN.md) M8/M9, [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) 완료 기준에 Windows 항목 반영

## 7. 단계 4 — 수동 검증 체크리스트 (Windows)

- [ ] `install.ps1`로 설치 → 실행 (콘솔 창 없음, 작업표시줄·창 아이콘)
- [ ] git 미설치 상태에서 안내 배너 → 경로 지정 후 정상 동작
- [ ] 저장소 열기: 드라이브 문자, UNC, 한글·공백 경로
- [ ] 상태 / diff(CRLF 파일 포함) / 스테이지 / 커밋 / 푸시(HTTPS, Git Credential Manager) / pull / 브랜치·태그
- [ ] 한글 IME 입력(커밋 메시지, 프롬프트, 검색창)
- [ ] 단축키: Ctrl 조합 동작, 표시 문구(`Ctrl+…`), Ctrl+Tab 탭 전환 스위처, Ctrl+Backspace
- [ ] "reveal in File Explorer", 폴더 선택 대화상자
- [ ] 파일 감시(외부 편집기에서 파일 수정 시 자동 갱신)
- [ ] 설정·로그가 `%APPDATA%`에 생기는지
- [ ] 고DPI·다중 모니터, 창 크기 복원
- [ ] 제거(폴더 삭제) 후 잔여물

## 8. 리스크

| 리스크 | 대응 |
|---|---|
| 한글 IME (winit 포크가 macOS용) | 단계 0에서 가장 먼저 확인 |
| onig C 빌드 | `fancy-regex`로 전환 |
| GPU 백엔드(DX12/Vulkan) | tiny-skia 폴백 검토 |
| 글리프 폴백 누락 | `keys.rs`에 대체 매핑 |
| 숨은 Unix 가정 테스트 | 로컬 `cargo test`로 반복 수정 |
| 서명 없는 exe의 SmartScreen 경고 | 수용, 문서로 안내. 후속으로 Azure Trusted Signing·MSI·winget |

## 9. 완료 기준

- Windows 로컬 + CI(`windows-latest`)에서 `cargo fmt` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` 통과
- CI macOS 잡 통과, macOS 단축키 표시는 기존과 동일 (`keys::local`은 macOS에서 항등)
- `release-<version>` 태그 드라이런으로 zip·sha256 자산 생성 확인 → 깨끗한 Windows에서 `install.ps1` 실행
- 7장 체크리스트 완료
