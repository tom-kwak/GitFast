# gf — 에이전트 안내

gf(Git Fast)는 Rust + iced 0.14로 만든 Git GUI 클라이언트입니다. macOS와 Windows를 지원하고, Linux는 예정입니다.
쓰기와 실행은 시스템 `git` CLI로 하고, 빠르고 결과가 같은 조회와 ref 편집은 gix로 합니다.

이 파일은 모든 에이전트(Claude Code, Codex 등)가 읽는 단일 원본입니다. `CLAUDE.md`는 이 파일을 불러오기만 합니다.

## 읽을 문서

| 언제 | 문서 |
|---|---|
| 모든 작업 전 | [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): 구조, 피처 템플릿, UI·키보드·OS·성능 규칙, 테스트, 완료 기준 |
| 화면·상호작용을 설계할 때 | [PLAN.md](PLAN.md) §4-1 터미널 스타일, §4-2 상호작용 원칙 |
| 다음에 무엇을 만들지 볼 때 | [PLAN.md](PLAN.md) §8 마일스톤 |
| git 명령을 추가·변경할 때 | [docs/GIX_MIGRATION.md](docs/GIX_MIGRATION.md): 명령별 백엔드(gix/CLI) 표와 계약 테스트 |
| 메모리를 잴 때 | [tools/mem-bench/README.md](tools/mem-bench/README.md) |
| 릴리스할 때 | [README.md](README.md) "Releasing", `docs/release-notes/<version>.md` |

문서와 코드가 다르면 **코드가 기준**입니다. 다른 점을 발견하면 같은 작업에서 문서를 고칩니다.

## 반드시 지킬 규칙

- **완료 기준**: `cargo fmt` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace`가 모두 통과해야 합니다. 새 로직에는 테스트를 붙입니다.
- **화면 확인**: 앱을 띄워 보지 않은 UI 변경은 결과 보고에 "화면 미확인"이라고 적습니다.
- **확인 다이얼로그**: 삭제·변경 작업은 공통 확인 다이얼로그 `App::confirm(Confirm::delete | Confirm::change)`를 거칩니다.
- **색**: 테마 토큰(`Tokens`)으로만 지정합니다. 색 상수를 직접 쓰지 않습니다.
- **툴바**: git 주요 명령만 두고, refresh는 맨 오른쪽에 둡니다. 패널 토글은 그 패널 헤더에 둡니다.
- **성능**(DEVELOPMENT.md §7)
  - 새로고침은 `request_parts`/`request_touched`로 요청만 합니다.
  - `view()`에서 목록 전체를 훑지 않습니다.
  - UI 스레드에서 git이나 큰 파일 I/O를 하지 않습니다.
- **운영체제별 코드**: gf-app 안에서는 `gf-app/src/platform/`에, 하위 크레이트에서는 `gf_core::tool_path`에 둡니다. 피처 코드에서 `cfg!`를 쓰지 않습니다(DEVELOPMENT.md §4-1).
- **단축키 문구**: mac 글리프(`⌘⇧Z`)로 쓰고 `widgets::mono`/`dim`으로 그립니다. Windows에서는 자동으로 `Ctrl+Shift+Z`가 됩니다.

## 작업 흐름

- **브랜치**: `develop`에서 `feature/<이름>`을 따서 작업하고 `develop`에 merge합니다. `main`은 릴리스용입니다. `release-<version>` 태그가 릴리스를 만듭니다.
- **커밋은 사용자가 요청할 때만** 합니다. 푸시도 같습니다.
  - 화면의 기능이나 상호작용을 바꾼 커밋이면, 커밋 직전에 UI 스윕(`cargo test -p gf-app -- --ignored sweep_`)을 돌립니다.
- **커밋 메시지**: 영어로 씁니다. 첫 줄은 사용자가 보는 동작을 설명하는 짧은 명령문입니다.
  - 예: `Hold back large diffs until asked, and refuse ones past 32 MiB`
  - 영역 접두어는 선택입니다: `gf-git: …`, `history: …`, `fix(tags): …`
  - 본문에는 무엇을 왜 바꿨는지 적습니다.
  - `Co-Authored-By`나 "Generated with …" 같은 에이전트 표기는 붙이지 않습니다. 작성자는 사용자 한 명입니다.
- **언어**
  - 사용자에게 하는 말과 `docs/`·`PLAN.md`·이 파일은 한국어로 씁니다.
  - 코드, 주석, 커밋 메시지, README, 릴리스 노트는 영어로 씁니다.
- **문서 갱신**: 동작을 바꾸면 같은 작업에서 관련 문서를 고칩니다.
  - git 명령의 백엔드 → GIX_MIGRATION.md §2 표
  - 규칙·구조 → DEVELOPMENT.md
  - 사용자에게 보이는 기능 → 릴리스할 때 README(측정값·기능 섹션)와 release notes. README에는 사실만 쓰고, 수치에는 출처와 측정 조건을 붙입니다

## 툴체인

- macOS: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`
- Windows: rustup 기본 경로(`%USERPROFILE%\.cargo\bin`). Git Bash에서는 `export PATH="$HOME/.cargo/bin:$PATH"`
- 앱을 다른 설정과 섞이지 않게 띄우려면 `GF_HOME=<임시 폴더> cargo run -p gf-app`을 씁니다.
