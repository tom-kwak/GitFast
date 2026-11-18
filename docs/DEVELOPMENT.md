# gf 개발 지침서 (피처 단위 개발)

사람이든 AI든 이 문서만 읽고 피처 하나를 끝까지 구현할 수 있도록 쓴 문서입니다.
제품 방향과 마일스톤은 [PLAN.md](../PLAN.md)를 기준으로 합니다. 이 문서와 PLAN.md가 다르면 **실제 코드가 기준**입니다.

---

## 0. 작업 시작 전 체크리스트

1. `PLAN.md`에서 §2 아키텍처 원칙, §4-1 터미널 스타일, §4-2 상호작용 원칙, §8 마일스톤을 읽습니다.
2. 이 문서의 §2 구조, §3 피처 템플릿, §7 성능 규칙을 읽습니다.
3. 비슷한 기존 피처를 하나 골라 그대로 따라 합니다.
   - 상태가 있는 목록 → `features/changes`
   - 순수 뷰 → `features/op_banner`
   - 메뉴 → `features/context_menu`
4. 툴체인 경로(macOS Homebrew rustup)를 잡습니다: `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`

## 1. 완료 기준 (모두 통과해야 "완료")

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- 새 로직(상태 변경, 파서, git 연산)에는 반드시 테스트를 붙입니다(§8).
- 버튼·메뉴·다이얼로그를 바꿨다면 UI 스윕도 돌립니다(몇 분 걸림, CI에서도 돕니다):
  `cargo test -p gf-app -- --ignored sweep_`.
  화면에 보이는 모든 글자를 헤드리스로 클릭(왼쪽·오른쪽)해, 눌러도 화면·저장소·포커스·클립보드
  어느 것도 바뀌지 않는 "조용한" 버튼을 찾습니다(`app/ui_sweep.rs`). 이미 고른 것을 다시 고르는
  클릭만 예외입니다. 할 일이 없는 버튼은 비활성(`on_press_maybe(None)`)으로 그립니다.
  `GF_SWEEP_DEPTH=2`로 열린 화면에서 열리는 화면까지 훑습니다(약 10분).
- UI는 테스트로 확인할 수 없는 부분(배치, 색, 실제 키 입력)이 있습니다. 앱을 띄워 확인하지 않았다면 결과 보고에 **"화면 미확인"**을 명시합니다.
- 커밋은 사용자가 요청할 때만 합니다.

---

## 2. 구조

### 2.1 크레이트 (UI와 코어 완전 분리)

| 크레이트 | 역할 | 규칙 |
|---|---|---|
| `gf-core` | 도메인 모델(`model::{Commit, Oid, Branch, StatusEntry, FileDiff…}`), `GfError`/`Result`, 이벤트 버스(`CoreEvent`, `EventBus`) | UI 의존성 금지 |
| `gf-git` | git 백엔드. `backend/`는 영역별 인터페이스(trait)와 `CliBackend`, `ops/*.rs`는 영역별 타입·파서·CLI 구현, `routes.rs`는 명령별 백엔드 선택, `gix_reader.rs`는 gix 조회 | 명령별 백엔드는 [GIX_MIGRATION.md](GIX_MIGRATION.md) 표를 따릅니다. 모든 명령은 CLI 구현을 가집니다 |
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
├─ app/              루트 App (라우팅만)
│  ├─ mod.rs         Message, update, view, 전역 키 → KeyInput
│  ├─ outputs.rs     피처 Output 처리(on_history, on_sidebar …)
│  ├─ ops.rs         git 작업 실행·완료 처리, undo 기록(Record → op_recorded)
│  ├─ advanced.rs    merge/rebase/cherry-pick/bisect
│  ├─ undoing.rs     ⌘Z/⌘⇧Z: 가드 확인 → (확인창) → 복원, undo 목록
│  ├─ dialogs.rs     공통 확인 다이얼로그(Confirm, Purpose)
│  └─ focus.rs       키 라우팅(메뉴→다이얼로그→입력창→패널), 패널 테두리
├─ features/<name>/mod.rs   피처 하나 = 폴더 하나
├─ widgets/          재사용 위젯(virtual_list, pointer, graph_canvas, format …)
├─ theme/            색·폰트 토큰(Tokens), 스타일 함수(style.rs), 프리셋
└─ highlight/        syntect 구문 강조
```

---

## 3. 피처 템플릿

### 3.1 규칙

- 피처는 **다른 피처를 직접 건드리지 않습니다.** 피처 밖의 일(git 실행, 다른 피처 변경, 다이얼로그)은 `Output`으로 요청하고 App이 처리합니다.
- 상태가 있으면 `Feature`를 구현합니다. 상태가 없으면 `pub fn view(...)` 함수만 둡니다.
- 비동기 작업은 `Update::task(Task)`로 돌려줍니다. UI 스레드에서 git이나 파일 I/O를 하지 않습니다.

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
6. **툴바에는 git 주요 명령만** 두고 refresh는 맨 오른쪽에 둡니다. 패널 토글은 그 패널 안에 둡니다.
7. **삭제·변경은 공통 확인 다이얼로그를 거칩니다.**
   - `App::confirm(Confirm::delete(..) | Confirm::change(key, ..))`를 쓰고, 실행할 작업은 `dialogs::Purpose`에 variant로 추가합니다.
   - `delete`(빨강, 클릭 필수): discard, 브랜치·태그·stash 삭제, force push.
   - `change`(노랑, "다시 묻지 않기" 가능): checkout, pull/push, stash apply, amend 등.
   - 확인 없이 실행: stage/unstage, 사용자가 프롬프트에 직접 입력한 명령.
8. **우클릭/액션 메뉴**는 `features/context_menu`를 재사용합니다.
   - `Menu { title, entries: vec![Entry::item(key, label, hint, msg), Entry::Separator, Entry::caution(..)] }`를 `Output`으로 넘깁니다.
   - App은 `open_menu(menu.map(Message::X))`로 엽니다.
9. **긴 작업은 비동기**로 처리합니다. 진행 표시는 스피너와 상태줄, 결과는 명령 로그와 flash에 남깁니다.

---

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
2. **필요한 부분만 다시 읽습니다.**
   - 바뀐 것에 맞는 `Parts`만 요청합니다. 히스토리는 `history_key`가 같으면 다시 읽지 않습니다.
   - 열린 작업 트리 diff는 그 파일이 바뀌었을 때만(`Touched::includes`) 다시 읽습니다.
   - 같은 데이터를 다시 읽는 반복 작업에는 캐시를 둡니다. 키는 내용이 같음을 보장하는 값(크기+수정 시각, `Arc` 동일성)으로 정합니다. 예: untracked 줄 수(`count_files`), ignore 판정(`gf_watch::IgnoreCache`).
3. **`view()`에서는 화면에 보이는 것만 계산합니다.** iced는 메시지마다 화면 전체를 다시 만듭니다(스피너 동작 중에는 초당 10번).
   - 목록 전체를 훑는 계산은 로드할 때 백그라운드에서 끝내 데이터에 담습니다. 예: `Snapshot.badges`, `LoadedDiff::new`(줄 너비·하이라이트).
   - 그럴 수 없으면 `widgets::memo::Memo`에 담습니다.
   - 긴 목록은 `virtual_list`로 그립니다.
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
   - 최적화는 결과가 같다는 테스트와 함께 넣습니다.

## 8. 테스트 작성법

- **gf-app**: UI 없이 `State::update` / `nav`를 호출하고 `Update.output` 또는 `Moved.message`를 `matches!`로 검사합니다.
  - 토큰이 필요하면 `crate::theme::preset("gf-dark")`를 씁니다.
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
참고: docs/DEVELOPMENT.md(§7 성능 규칙 포함), PLAN.md §<번호>, 비슷한 기존 피처 <경로>
범위: <건드릴 파일/크레이트>, 범위 밖은 수정 금지
완료 기준: §1 명령 3개 통과 + 새 로직 테스트 + 화면 확인 여부 보고
금지: 커밋/푸시, 확인 다이얼로그 생략, 토큰 외 색상
```
