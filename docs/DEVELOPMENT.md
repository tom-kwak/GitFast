# gf 개발 지침서 (피처 단위 개발)

사람이든 AI든 이 문서만 읽고 피처 하나를 끝까지 구현할 수 있도록 쓴 문서입니다.
작업 흐름(브랜치, 커밋 메시지, 언어)은 [AGENTS.md](../AGENTS.md), 제품 방향과 마일스톤은 [PLAN.md](../PLAN.md)를 따릅니다. 문서끼리 또는 문서와 코드가 다르면 **실제 코드가 기준**입니다.

---

## 0. 작업 시작 전 체크리스트

1. `PLAN.md`에서 §2 아키텍처 원칙, §4-1 터미널 스타일, §4-2 상호작용 원칙을 읽습니다.
2. 이 문서의 §2 구조, §3 피처 템플릿, §7 성능 규칙을 읽습니다.
3. 비슷한 기존 피처를 하나 골라 그대로 따라 합니다.
   - 상태가 있는 목록 → `features/changes`
   - 순수 뷰 → `features/op_banner`
   - 메뉴 → `features/context_menu`
   - 우클릭 메뉴와 확인 작업이 많은 패널 → `features/worktrees` + `app/worktree_ops.rs`
4. 툴체인 경로를 잡습니다([AGENTS.md](../AGENTS.md) "툴체인").

## 1. 완료 및 커밋 기준

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- 새 로직(상태 변경, 파서, git 연산)에는 반드시 테스트를 붙입니다(§8).
- 화면의 기능이나 상호작용을 바꾼 작업의 커밋을 사용자가 요청했을 때만, 커밋 직전에 UI 스윕을 돌립니다.
  문구, 색상, 간격처럼 동작에 영향이 없는 사소한 변경은 건너뜁니다. 스윕 명령은 다음과 같습니다.
  `cargo test -p gf-app -- --ignored sweep_`.
  화면에 보이는 모든 글자를 헤드리스로 클릭(왼쪽·오른쪽)해, 눌러도 화면·저장소·포커스·클립보드
  어느 것도 바뀌지 않는 "조용한" 버튼을 찾습니다(`app/ui_sweep.rs`). 이미 고른 것을 다시 고르는
  클릭만 예외입니다. 할 일이 없는 버튼은 비활성(`on_press_maybe(None)`)으로 그립니다.
  `GF_SWEEP_DEPTH=2`로 열린 화면에서 열리는 화면까지 훑습니다(약 10분).
  `GF_SWEEP_PROFILE=1`이면 스윕마다 끝날 때 단계별 시간(fixture 복사, boot, open, 화면 재진입,
  렌더링, 상태·저장소 비교, press 등)을 출력합니다(`app/ui_sweep/profile.rs`). 출력을 보려면
  `-- --ignored --nocapture sweep_history`처럼 `--nocapture`를 붙입니다.
  스윕은 화면을 `app/ui_sweep/sim.rs`로 그립니다. 렌더러를 테스트 스레드당 하나만 만들고, `Ui`마다
  위젯 상태를 유지합니다(실제 앱처럼 글자 shaping을 한 번만 합니다). `press`(클릭 미리보기)는 위젯
  상태를 따로 둬서 다음 실제 클릭이 더블클릭으로 잡히지 않게 합니다. 준비 단계가 있는 스윕은
  `Ui::open_prepared`로 준비된 저장소를 한 번만 만들고 이후에는 복사해 씁니다.
- UI는 테스트로 확인할 수 없는 부분(배치, 색, 실제 키 입력)이 있습니다. 앱을 띄워 확인하지 않았다면 결과 보고에 **"화면 미확인"**을 명시합니다.
- 커밋은 사용자가 요청할 때만 합니다.

---

## 2. 구조

### 2.1 크레이트 (UI와 코어 완전 분리)

| 크레이트 | 역할 | 규칙 |
|---|---|---|
| `gf-core` | 도메인 모델(`model::{Commit, Oid, Branch, StatusEntry, FileDiff…}`), `GfError`/`Result`, 이벤트 버스(`CoreEvent`, `EventBus`) | UI 의존성 금지 |
| `gf-git` | git 백엔드. `backend/`는 영역별 인터페이스(trait)와 `CliBackend`, `backend/gix_*.rs`는 gix 구현, `ops/*.rs`는 영역별 타입·파서·CLI 구현, `routes.rs`는 명령별 백엔드 선택, `cli/`는 실행기(`GitRunner`)와 porcelain 파서 | 명령별 백엔드는 [GIX_MIGRATION.md](GIX_MIGRATION.md) 표를 따릅니다. 모든 명령은 CLI 구현을 가집니다 |
| `gf-config` | 설정(`Settings`), 경로(`AppPaths`) | serde + toml |
| `gf-watch` | 파일 감시 → `CoreEvent` | |
| `gf-test-support` | 테스트용 임시 저장소 | |
| `gf-app` | iced 0.14 앱(바이너리) | 코어를 부르기만 하고 git 로직을 넣지 않습니다 |
| `gf-commands`, `gf-flow`, `gf-hosting*`, `gf-auth` | M5~M7용 자리(현재 비어 있음) | 사용 시작 시 루트 `Cargo.toml`의 주석을 풉니다 |

### 2.2 gf-app 내부

```
src/
├─ main.rs           모듈 선언
├─ feature.rs        Feature trait, Update<M, O>, Cx   ← 피처 계약
├─ nav.rs            키보드 패널 이동(Pane, Nav, step/reveal)
├─ repo_tab.rs       저장소 탭별 상태(RepoTab, Snapshot)
├─ undo.rs           undo/redo 기록(Step, History — git dir의 gf/undo.json에 저장)
├─ git_actions.rs    git 실행 헬퍼
├─ badges.rs         커밋별 브랜치·태그 배지(로드할 때 한 번 계산)
├─ logging.rs        tracing 설정(stderr + 날짜별 로그 파일)
├─ platform/         운영체제별 차이를 모두 모은 곳(§4-1)
│  ├─ mod.rs         홈 폴더, 경로 구분자, 파일 관리자(reveal), 브라우저, 창 설정
│  └─ keys.rs        단축키 표기: macOS `⌘⇧K` → 그 외 `Ctrl+Shift+K`
├─ app/              루트 App (라우팅만)
│  ├─ mod.rs         Message, update, view, 전역 키 → KeyInput
│  ├─ outputs.rs     피처 Output 처리(on_history, on_sidebar …)
│  ├─ ops.rs         git 작업 실행·완료 처리, undo 기록(Record → op_recorded)
│  ├─ advanced.rs    merge/rebase/cherry-pick/bisect
│  ├─ undoing.rs     ⌘Z/⌘⇧Z: 가드 확인 → (확인창) → 복원, undo 목록
│  ├─ dialogs.rs     공통 확인 다이얼로그(Confirm, Purpose)
│  ├─ focus.rs       키 라우팅(메뉴→다이얼로그→입력창→패널), 패널 테두리
│  ├─ worktree_ops.rs  worktrees 패널의 Output 처리(열기, 제거, merge into base, update from base …)
│  ├─ branch_delete.rs 여러 ref를 골라 한 번에 삭제
│  ├─ reveal.rs      blame 등에서 커밋을 히스토리에서 찾아 선택·스크롤
│  ├─ settings.rs    설정 화면(섹션별) → 공유 설정, 서버 로그인(ACCOUNTS: 토큰 상태·확인)과 인증 실패 시 로그인 후 재실행
│  ├─ clone.rs       open 창의 URL clone(폴더 묻기 → 백그라운드 clone, 상태 줄 진행률·Esc 취소 → 탭으로 열기)
│  ├─ ui_sweep.rs    헤드리스 앱 드라이버(`Ui`)와 UI 스윕(§1, §8)
│  └─ regression_tests.rs, keyboard_tests.rs   실제 App으로 돌리는 회귀 테스트
├─ features/<name>/mod.rs   피처 하나 = 폴더 하나
├─ widgets/          재사용 위젯(virtual_list, pointer, reorder(끌어서 순서 바꾸기), graph_canvas, format …)
├─ theme/            색·폰트 토큰(Tokens), 스타일 함수(style.rs), 프리셋
└─ highlight/        syntect 구문 강조
```

---

## 3. 피처 템플릿

### 3.1 규칙

- 피처는 **다른 피처를 직접 건드리지 않습니다.** 피처 밖의 일(git 실행, 다른 피처 변경, 다이얼로그)은 `Output`으로 요청하고 App이 처리합니다.
- 상태가 있으면 `Feature`를 구현합니다. 상태가 없으면 `pub fn view(...)` 함수만 둡니다.
- 비동기 작업은 `Update::task(Task)`로 돌려줍니다. UI 스레드에서 git이나 파일 I/O를 하지 않습니다.

히스토리 Find는 파일과 커밋 모드에서 같은 검색창과 목록 키를 씁니다. 파일 검색은 기준 커밋의 트리를 조회합니다. 커밋 검색은 현재 그래프 범위를 사용하며 `app/history_search.rs`가 저장소·검색 모드·범위·요청 세대를 확인합니다. 검색어, 모드, 범위를 바꿀 때 기존 커밋 조회는 취소합니다. 선택한 커밋이 화면에 로드되기 전이면 현재 범위의 이력을 늘려 그 위치를 표시합니다. 검색 규칙과 화면 예시는 [히스토리 검색 설계안](HISTORY_SEARCH.md)에 있습니다.

diff 로딩을 바꿀 때는 `features/diff_view/loading.rs`에서 요청 생성과 결과 수락 조건을 함께 확인합니다. App은 `RepoId`와 패널 역할로 결과를 전달합니다. 같은 파일을 다시 읽어도 최신 요청만 본문을 교체하며, 추가 구문 강조는 계산에 사용한 diff에만 붙입니다. `cargo test -p gf-app features::diff_view`로 응답 순서, 선택·복사, 부분 stage를 검증합니다.

### 3.2 코드 뼈대

```rust
//! 한두 문단: 이 피처가 무엇을 보여주고 무엇을 하는지.

use iced::Element;

use crate::feature::{Cx, Feature, Update};
use crate::theme::Tokens;
use crate::widgets;

#[derive(Debug, Default)]
pub struct State { /* 선택, 스크롤(ScrollPos), 접힘 … */ }

#[derive(Debug, Clone)]
pub enum Message { /* 사용자 입력 */ }

#[derive(Debug, Clone)]
pub enum Output { /* App에 요청할 것 */ }

impl Feature for State {
    type Message = Message;
    type Output = Output;

    fn update(&mut self, message: Message) -> Update<Message, Output> {
        match message { /* 상태 변경 → Update::none() / ::output(..) / ::task(..) */ }
    }

    fn view<'a>(&'a self, cx: Cx<'a>) -> Element<'a, Message> {
        let t = cx.tokens;          // 색·크기는 반드시 토큰에서
        let Some(snap) = cx.snapshot else {
            return widgets::panel(t, "TITLE".into(), widgets::empty_hint(t, "loading…"));
        };
        todo!()
    }
}

#[cfg(test)]
mod tests { /* update/nav 단위 테스트 */ }
```

- update에 스냅샷이 필요하면 `update_with(msg, Option<&Snapshot>)`를 따로 둡니다(예: `history`, `sidebar`).

### 3.3 App에 연결하기 (순서대로)

1. `features/mod.rs`에 `pub mod <name>;`을 추가합니다.
2. 상태 필드를 둡니다. 저장소마다 다르면 `RepoTab`에, 앱 전체에 하나면 `App`에 둡니다.
3. `app/mod.rs`의 `Message`에 `<Name>(<name>::Message)` variant를 추가하고 `update`에서 라우팅합니다.
   - 기존 `Message::Changes(m)` arm을 그대로 복사하면 됩니다: `update` → `task.map(Message::X)` → output이 있으면 `self.on_x(o)`.
4. `app/outputs.rs`에 `on_<name>(Output)`을 만듭니다. git 실행·다이얼로그·다른 피처 갱신은 여기서 합니다.
5. `view`(또는 `main_area`)에 배치하고 `.map(Message::X)`로 감쌉니다.
6. 키보드로 조작하는 패널이면 §5를 따릅니다.

---

## 4. UI 규칙 (위반 시 리뷰 반려)

1. **터미널 스타일**
   - 고정폭 폰트만 씁니다. 둥근 모서리, 그림자, 그라디언트는 쓰지 않습니다.
   - 텍스트형 버튼을 씁니다: `widgets::text_button`, `icon_button`, `danger_button`.
   - 패널은 `widgets::panel` / `accent_panel` / `panel_with_action`으로 만듭니다.
2. **색은 토큰만** 씁니다. 색 상수를 직접 쓰지 않습니다.
   - 의미 색: `t.added()`, `t.removed()`, `t.warning()`, `t.error()`, `t.accent()`, `t.branch()`, `t.tag()`, `t.head()`, `t.dim`
   - 파일 상태 글자 색은 반드시 `widgets::change_color(t, code)`로 정합니다. 워킹트리와 히스토리가 같은 색이어야 합니다.
3. **긴 목록은 `widgets::virtual_list::virtual_list`**를 씁니다.
   - 줄 높이는 함수 하나(`row_height(t)`)로 정의해 뷰와 키보드 이동이 같은 값을 씁니다.
   - 스크롤 id는 `const` 하나로 둡니다.
4. **빈 상태는 안내로** 채웁니다: `widgets::empty_hint(t, "무엇을 하면 채워지는지")`.
5. **단축키는 발견 가능하게** 합니다. 툴팁(`widgets::tip`)과 `:help`(`app/outputs.rs`의 help 줄)에 적습니다.
   - 문구에는 **mac 글리프로** 씁니다(`⌘⇧Z`, `⌃⇥`, `⏎`). 화면에 그릴 때 `widgets::mono`/`dim`이 `platform::keys::local`을 거쳐 macOS 밖에서는 `Ctrl+Shift+Z`로 바꿉니다. `text()`를 직접 부르지 말고 이 헬퍼를 씁니다.
6. **툴바에는 git 주요 명령만** 두고 refresh는 맨 오른쪽에 둡니다. 패널 토글은 그 패널 안에 둡니다.
7. **삭제·변경은 공통 확인 다이얼로그를 거칩니다.**
   - `App::confirm(Confirm::delete(..) | Confirm::change(key, ..))`를 쓰고, 실행할 작업은 `dialogs::Purpose`에 variant로 추가합니다.
   - `delete`(빨강, 클릭 필수): discard, 브랜치·태그·stash 삭제, force push.
   - `change`(노랑, "다시 묻지 않기" 가능): checkout, pull/push, stash apply, amend 등.
   - 일반 Pull은 fast-forward만 바로 실행합니다. 브랜치가 갈라졌거나 로컬 파일과 겹치면 확인을 받은 뒤 통합합니다. 일반 Push는 바로 실행하고, 알려진 거절 가능성과 원격 ref를 강제로 변경하는 설정은 확인을 거칩니다.
   - 확인 없이 실행: stage/unstage, 사용자가 프롬프트에 직접 입력한 명령.
8. **우클릭/액션 메뉴**는 `features/context_menu`를 재사용합니다.
   - `Menu { title, entries: vec![Entry::item(key, label, hint, msg), Entry::Separator, Entry::caution(..)] }`를 `Output`으로 넘깁니다.
   - App은 `open_menu(menu.map(Message::X))`로 엽니다.
9. **긴 작업은 비동기**로 처리합니다. 진행 표시는 스피너와 상태줄, 결과는 명령 로그와 flash에 남깁니다.
10. **안내 문구**(다이얼로그·툴팁·메뉴 힌트·빈 상태)는 처음 보는 사람이 한 번에 이해하게 씁니다.
    - **화면에 보이는 이름만** 씁니다: 브랜치 이름, 폴더 경로(`widgets::format::tilde`). "main tree", "linked" 같은 내부 용어는 쓰지 않습니다. `main` 브랜치와 헷갈리는 말도 피합니다.
    - **같은 것은 같은 말로** 부릅니다: 작업 트리 하나는 *worktree*, 그 디렉터리는 *folder*, 작업이 돌아갈 브랜치는 *base*입니다. *tree*만 따로 쓰지 않습니다.
    - **다이얼로그 본문은 2줄 이내**로 씁니다. 1줄은 무슨 일인지, 2줄은 결과(사라지는 것·남는 것)입니다. 그 경우에만 해당하는 경고(예: 커밋하지 않은 변경 N개)는 한 줄 더 붙일 수 있습니다. 버튼이 답이 되도록 질문을 반복하지 않습니다.
    - **버튼은 하는 동작**으로 씁니다(`Remove`, `View detached`, `Open that worktree`). 체크박스는 켰을 때 하는 일을 씁니다.
    - **개수는 `widgets::format::count`**로 씁니다(`1 commit`, `3 commits`). `(s)`를 붙이지 않습니다.
    - 툴팁·메뉴 힌트는 소문자로 시작하는 짧은 구로, 그 동작의 결과를 말합니다(`delete its folder; the branch stays`).

---

### 4-1. 운영체제별 차이 (macOS · Windows)

- 운영체제마다 달라지는 동작은 **gf-app 안에서는 `gf-app/src/platform/`에, 하위 크레이트(gf-git, gf-watch …)에서는 `gf_core::tool_path`에** 둡니다. 피처 코드(`features/`, `app/`, `widgets/`)에서 `cfg!(target_os = …)`·`cfg!(windows)`를 쓰지 않고 `platform::`의 함수를 부릅니다.
  - 홈 폴더 `platform::home_dir()`(`HOME`을 직접 읽지 않음), 입력 경로 구분자 `is_path_sep`, 경로처럼 보이는 입력 `looks_like_path`
  - 파일 관리자 `reveal`·`FILE_MANAGER`("reveal in {FILE_MANAGER}"), 브라우저 `open_url`, 창 설정 `window_settings`, 글꼴 `mono_font`
  - 단축키 표기 `platform::keys::local`. `widgets::mono`/`dim`은 자동으로 거치지만, `text_input` placeholder처럼 이 헬퍼를 지나지 않는 문구는 직접 변환합니다.
- git·도구 실행 환경은 `gf_core::tool_path`에 있습니다. git을 동기로 부를 때는 `tool_path::git_command()`(설정의 git 경로 + 확장 PATH + Windows 콘솔 창 숨김)를 씁니다. 새 서브프로세스를 만들면 `hide_console`을 거칩니다.
  - 예외: tokio `Command`를 쓰는 gf-git runner는 `hide_console`(std 전용)을 쓸 수 없어 `#[cfg(windows)]`로 `CREATE_NO_WINDOW`를 직접 넣고, gf-test-support는 테스트 헬퍼라 `cfg`를 직접 씁니다.
- 저장소 안 경로를 화면에 쓸 때는 `widgets::format::repo_path`(항상 `/`)를 씁니다. `Path::join` 결과의 `display()`는 Windows에서 `\`가 섞입니다.
- 테스트도 두 OS에서 돌아야 합니다: `/tmp` 같은 고정 경로 대신 `tempfile`을 쓰고, 플랫폼 전용 테스트에는 `#[cfg(...)]`와 이유 주석을 붙입니다. CI는 macOS와 Windows에서 돕니다. UI 스윕은 macOS에서만 돕니다. 이미 같은 코드(`*.md`·`docs/` 제외)로 통과한 검사는 `scripts/ci-verified.sh`가 판정해서 다시 돌리지 않습니다(release.yml의 CI·스윕). UI 스윕과 패키징은 릴리스 태그를 만들 때만 돕니다.
- Windows에서 알아둘 차이
  - 실행 비트는 항상 `false`로 기록되고, undo의 심볼릭 링크 복원은 `Unsupported`입니다.
  - `GIT_EDITOR`·`GIT_SEQUENCE_EDITOR`는 Git for Windows의 내장 sh로 실행됩니다. 에디터 명령을 바꾸면 백슬래시·한글·공백 경로에서 interactive rebase(reword/squash)를 확인합니다.
  - 폴더를 실제 경로로 바꿀 때는 `std::fs::canonicalize` 대신 `platform::canonicalize`를 씁니다. Windows의 `\\?\` 접두어를 지워서 git·탭의 경로와 같아집니다(테스트는 `gf_test_support::without_verbatim_prefix`).
- Windows 개발 환경: Visual Studio Build Tools 2022(C++ 데스크톱 개발, onig 빌드에 필요), rustup stable, Git for Windows. 저장소는 짧은 경로에 두고 `core.longpaths true`, `core.autocrlf false`를 켭니다(줄끝은 `.gitattributes`가 정합니다).

## 5. 키보드 (패널 포커스 모델)

- **키를 받는 순서**(`app/focus.rs`): 열린 메뉴 → 다이얼로그 → 포커스된 텍스트 입력창 → 활성 패널(`Pane`) → App 기본 동작.
  - App 기본 동작: `←/→/Enter` 패널 이동, `⇥` 패널 순환, `/` 프롬프트.
- **새 패널을 키보드로 조작하게 하려면** 아래 순서로 연결합니다.
  1. `nav::Pane`에 variant를 추가하고 `App::panes()`에 화면 순서대로 넣습니다.
  2. 피처에 `pub fn nav(&self, nav: Nav, …, t: &Tokens) -> Option<Moved<Message>>`를 구현합니다.
     - 쓰지 않는 키는 `None`을 돌려줍니다(App이 처리).
     - 키는 썼지만 할 일이 없으면 `Some(Moved::stay())`를 돌려줍니다.
     - 이동에는 `nav::step(current, len, key, nav::page(scroll, h))`를 씁니다.
     - 선택이 보이게 하려면 `Moved::message(msg).with_task(nav::reveal(LIST_ID, scroll, row, h))`를 씁니다.
  3. `focus.rs`의 `nav_pane`에 arm을 추가합니다.
  4. `view`에서 `focus::pane_frame(t, Pane::X, active, element)`로 감쌉니다.
  5. 클릭으로도 활성 패널이 바뀌도록, 해당 피처의 선택 메시지에서 `self.focus_pane(Pane::X)`를 호출합니다.
- **텍스트 입력창에 id를 줘야 하는 경우**: `.id(CONST)`를 주면 `pointer.field_at_key()`로 포커스를 구분할 수 있습니다. 예: `REFS_FILTER_ID`.
- **글자 키**는 물리 키 기준(`to_latin`)으로 처리해서 한글 입력 상태에서도 동작해야 합니다.
- **프롬프트로 포커스를 돌릴 때**는 직접 `operation::focus(PROMPT_ID)`를 부르지 말고 `self.restore_focus()`를 씁니다. 사용자가 프롬프트에 입력하던 중일 때만 돌아갑니다.

---

## 6. git 연산 추가 (gf-git)

1. 인터페이스: `crates/gf-git/src/backend/mod.rs`의 해당 영역 trait(예: `BranchOps`)에 메서드를 추가합니다. 새 영역이면 trait를 새로 만들고 `ops/<topic>.rs`를 `ops/mod.rs`에 등록합니다.
   - 형태: `fn x(&self, …) -> impl Future<Output = Result<T>> + Send;`
2. CLI 구현: `ops/<topic>.rs`의 `impl XxxOps for CliBackend`에 `async fn x`로 구현합니다.
   - **읽기**: `self.read(self.command([...]))` → 바이트를 받아 파서에 넘깁니다.
   - **쓰기**: `self.exec(self.command([...]).arg(..))`
   - 사용자 입력이 ref 자리에 들어가면 그 앞에 `--end-of-options`를 붙입니다.
   - 같은 파일의 보조 함수는 `impl CliBackend { … }`에 둡니다.
3. 연결: `crates/gf-git/src/routes.rs`에 `Repository`의 `pub async fn x`를 추가해 백엔드를 고릅니다(기본은 `XxxOps::x(&self.cli, …)`). gix 구현을 추가할 때는 [GIX_MIGRATION.md](GIX_MIGRATION.md) §4를 따릅니다.
4. 파서는 순수 함수 `pub fn parse_x(out: &[u8]) -> Result<…>`로 만들고, 실제 git 출력 예시로 단위 테스트합니다(예: `ops/worktree.rs`).
5. 실제 저장소가 필요한 시나리오는 `crates/gf-git/tests/*.rs`에 `gf-test-support`의 임시 저장소로 테스트합니다(예: `advanced_ops.rs`).
6. 새 모델 타입이 여러 크레이트에서 쓰이면 `gf-core::model`에 둡니다.

## 7. 성능 규칙 (가볍고 빠르게 — 위반 시 리뷰 반려)

gf는 성능이 가장 중요합니다. 새 코드는 아래 규칙을 지킵니다. 규칙마다 기준이 되는 기존 코드가 있으니 그대로 따라 합니다.

1. **새로고침은 요청만 합니다.**
   - 저장소 데이터를 다시 읽을 때는 `App::request_parts(id, Parts::…)`를 부릅니다.
   - 감시기가 바뀐 파일을 알 때는 `App::request_touched(id, parts, Touched::paths(..))`를 부릅니다.
   - `load_snapshot`을 직접 부르지 않습니다. 실제 로드는 매 메시지 뒤에 도는 `App::load_due`가 정합니다(`app/ops.rs`).
     - 탭당 로드는 하나만 돕니다.
     - 화면에 보이는 탭이 먼저 읽습니다. 복원된 다른 탭은 하나씩 첫 로드를 합니다.
     - 백그라운드 탭은 파일·index 변경을 탭이 보일 때까지 미룹니다. refs·config 변경은 바로 읽습니다(탭에 브랜치 이름이 보이므로).
     - 같은 저장소의 다른 워크트리 탭은 refs·워크트리 목록을 공유하지만, 감시기는 탭마다 자기 git dir만 봅니다. 한 탭에서 refs가 바뀌면(작업 완료, 감시기의 `RefsChanged`) `App::request_shared_refs`가 다른 트리의 탭에 `shared_refs_stale`만 표시하고, 그 탭이 앞으로 올 때 refs를 읽습니다. 백그라운드 탭의 이름은 자기 HEAD라서 미뤄도 됩니다(`Repository::common_dir`로 같은 저장소를 판별).
2. **필요한 부분만 다시 읽습니다.**
   - 바뀐 것에 맞는 `Parts`만 요청합니다. 히스토리는 `history_key`가 같으면 다시 읽지 않습니다.
   - 열린 작업 트리 diff는 그 파일이 바뀌었을 때만(`Touched::includes`) 다시 읽습니다.
   - 같은 데이터를 다시 읽는 반복 작업에는 캐시를 둡니다. 키는 내용이 같음을 보장하는 값(크기+수정 시각, `Arc` 동일성)으로 정합니다. 예: untracked 줄 수(`count_files`), ignore 판정(`gf_watch::IgnoreCache`).
3. **`view()`에서는 화면에 보이는 것만 계산합니다.** iced는 메시지마다 화면 전체를 다시 만듭니다(스피너 동작 중에는 초당 10번).
   - 목록 전체를 훑는 계산은 로드할 때 백그라운드에서 끝내 데이터에 담습니다. 예: `Snapshot.badges`, `LoadedDiff::new`(줄 너비·하이라이트).
   - 그럴 수 없으면 `widgets::memo::Memo`에 담습니다.
   - 긴 목록은 `virtual_list`로 그립니다.
   - 크기에 끝이 없는 출력은 상한을 둡니다. 예: diff는 `gf_git::DIFF_LIMIT`(32 MiB)를 넘으면 읽지 않고(`RunOptions::max_stdout`), 2만 줄이나 1 MiB를 넘는 파일은 ⏎를 누를 때까지 그리지 않습니다(`diff_view`의 `LARGE_DIFF_*`).
4. **canvas는 캐시합니다.** `canvas::Cache`를 쓰고, 내용으로 키를 잡습니다. virtual list는 스크롤할 때 위젯을 다른 행에 재사용하므로 위젯 기준 키는 틀립니다. 예: `widgets/graph_canvas.rs`의 `RowCache`.
5. **UI 스레드에서 git·큰 파일 I/O를 하지 않습니다.**
   - git 실행은 `Task::perform`으로 합니다.
   - 파일을 훑는 작업은 `tokio::task::spawn_blocking`으로 합니다.
6. **git 프로세스 수를 줄입니다.**
   - 빠르고 결과가 같으면 gix를 씁니다([GIX_MIGRATION.md](GIX_MIGRATION.md)).
   - 독립된 읽기는 `tokio::join!` / `join_all`로 동시에 실행합니다.
   - 반복 이벤트(감시기)마다 프로세스를 띄우지 않습니다.
7. **측정합니다.**
   - 로드 시간은 `RUST_LOG=gf=debug`의 `snapshot loaded` 줄에서 봅니다.
   - gix와 CLI 비교는 `tests/gix_bench.rs`로 합니다.
   - 메모리(탭 여러 개, 큰 diff, 깊은 히스토리)는 [tools/mem-bench](../tools/mem-bench/README.md)로 잽니다.
   - 다른 Git GUI(GitHub Desktop 등)와의 메모리 비교는 [tools/gui-bench](../tools/gui-bench/README.md)로 잽니다(macOS, 사람이 직접 조작).
   - 최적화는 결과가 같다는 테스트와 함께 넣습니다.

## 8. 테스트 작성법

- **gf-app 피처**: UI 없이 `State::update` / `nav`를 호출하고 `Update.output` 또는 `Moved.message`를 `matches!`로 검사합니다.
  - 토큰이 필요하면 `crate::theme::preset("gf-dark")`를 씁니다.
- **App 전체(회귀 테스트)**: `ui_sweep::Ui`로 실제 `App`을 창 없이 띄워 사용자처럼 조작하고 git 결과를 확인합니다.
  - `Ui::open_with(|repo| …)`로 fixture 저장소를 준비하고, `ui.click("label")`·`ui.send(Message::…)`로 조작하고, `ui.shows(..)`·`ui.flash()`로 확인합니다.
  - 저장소를 바꾸는 동작은 `app/regression_tests.rs`, 키 라우팅은 `app/keyboard_tests.rs`(`Message::KeyInput`)에 둡니다. 기존 기능을 바꾸기 전에 지금 동작을 고정하는 테스트를 먼저 추가합니다.
- **gf-git**: 파서는 실제 git 출력으로 단위 테스트하고, 시나리오는 `crates/gf-git/tests/`에서 `gf-test-support`의 `TestRepo`로 테스트합니다. gix 구현은 `tests/gix_contract.rs`에서 CLI와 결과가 같은지 비교합니다.
- **서버 연동(로그인·clone·push·fetch)**: 평소에는 `credential.helper = store --file=<임시 파일>`과 `url.<로컬 경로>.insteadOf`로 네트워크 없이 테스트합니다(`ui_sweep.rs`의 `with_store_helper`, `example_server`). 개발자의 키체인은 건드리지 않습니다.
  - 실제 GitLab으로 확인하려면 `app/ui_sweep/gitlab_e2e.rs`를 씁니다(`--ignored`). 비공개 프로젝트 `<user>/gf-demo`와 큰 `<user>/gf-big`(~150MB)이 필요합니다.
  - 실행: `GF_GITLAB_URL=http://localhost:8929 GF_GITLAB_USER=root GF_GITLAB_TOKEN=<PAT> cargo test -p gf-app gitlab_e2e -- --ignored --test-threads=1`
- **경로 비교**: `p.as_path() == Path::new("x")`처럼 씁니다. clippy `cmp_owned`를 피하기 위해서입니다.
- **테스트 이름**은 동작을 문장으로 씁니다. 예: `arrows_skip_headers_and_space_stages`.

## 9. 코드 스타일

- 모듈 맨 위에 `//!` 문서 주석으로 목적을 적습니다. 공개 항목에는 `///`로 "왜"를 적습니다.
- `unwrap()`은 테스트 외에 쓰지 않습니다(workspace lint). 실패 가능한 곳은 `?` 또는 `expect("이유")`를 씁니다.
- 한 줄 최대 100자(`rustfmt.toml`)입니다.
- 파일이 커지면(약 800줄 이상) 역할별 하위 모듈로 나눕니다. 예: `app/`가 `outputs.rs`, `advanced.rs`, `focus.rs`로 나뉘어 있습니다.

## 10. AI에게 작업을 맡길 때 프롬프트 형식

```
작업: <피처/수정 한 줄>
참고: AGENTS.md, docs/DEVELOPMENT.md(§7 성능 규칙 포함), PLAN.md §<번호>, 비슷한 기존 피처 <경로>
범위: <건드릴 파일/크레이트>, 범위 밖은 수정 금지
완료 기준: §1 명령 3개 통과 + 새 로직 테스트 + 화면 확인 여부 보고
금지: 커밋/푸시, 확인 다이얼로그 생략, 토큰 외 색상
```
