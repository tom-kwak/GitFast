# gf — 제품 계획

> 대상: macOS · Windows 데스크톱 앱(현재), Linux(예정)
> 언어: Rust (전 계층) · UI: iced 0.14
>
> 이 문서는 **제품 방향과 남은 계획**을 담습니다. 구조와 코딩 규칙은 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md), 작업 흐름은 [AGENTS.md](AGENTS.md)를 봅니다. 코드가 참조하는 절 번호(§4-1, §4-2, §10)는 바꾸지 않습니다.

---

## 1. 기술 선택

| 영역 | 선택 | 상태 | 이유 |
|---|---|---|---|
| UI | iced 0.14 (wgpu) | 사용 중 | 순수 Rust, 크로스플랫폼, Elm 구조(State/Message/update/view)라 피처를 모듈로 나누기 좋음. 그래프·diff는 canvas와 자체 위젯 |
| git 쓰기·실행 | 시스템 `git` CLI | 사용 중 | hooks, credential helper, LFS, 서명, `.gitconfig`가 사용자의 터미널과 같게 동작 |
| git 조회·ref 편집 | gix (gitoxide) | 사용 중 | 프로세스 없이 빠르게. 결과가 같을 때만 쓰고 안 되면 CLI로 폴백([docs/GIX_MIGRATION.md](docs/GIX_MIGRATION.md)) |
| 비동기 | tokio | 사용 중 | git 프로세스와 파일 I/O를 UI 스레드에서 분리 |
| 파일 감시 | notify | 사용 중 | 작업 트리가 바뀌면 필요한 부분만 다시 읽음 |
| 설정 | serde + toml, directories | 사용 중 | OS별 표준 경로(`GF_HOME`으로 한 폴더에 모을 수 있음) |
| 구문 강조 | syntect(Oniguruma) + two-face | 사용 중 | VS Code와 같은 TextMate 문법. `<config>/syntaxes/`의 `*.tmLanguage.json`·`*.sublime-syntax`를 자동 로드 |
| 로깅 | tracing | 사용 중 | 실행한 git 명령과 출력은 명령 로그 패널에 표시 |
| 패키징 | `scripts/package-macos.sh`(아키텍처별 .dmg: arm64·x86_64), `scripts/package-windows.ps1`(.zip) | 사용 중 | 설치는 `install.sh` / `install.ps1`, 릴리스는 `release.yml` |
| HTTP | reqwest (rustls) | M7 예정 | GitLab/GitHub API |
| 자격 증명 | keyring | M7 예정 | macOS Keychain / Windows Credential Manager. 지금은 git credential helper에 맡김 |
| 내장 터미널 | alacritty_terminal | M5 예정 | 구조화 UI로 못 하는 명령의 마지막 폴백 |

---

## 2. 아키텍처 원칙

1. **UI와 코어 완전 분리**: 코어 크레이트(gf-core, gf-git, gf-config, gf-watch)는 UI에 의존하지 않습니다. gf-app은 코어를 부르기만 합니다.
2. **trait 기반 확장 지점**: git 백엔드(영역별 `XxxOps`), 피처(`Feature`), 이후 호스팅 프로바이더를 trait으로 정의합니다. 새 기능은 새 파일/폴더 하나와 연결 몇 줄로 끝납니다.
3. **모든 실행은 하나의 Runner를 거침**: `GitRunner`가 로그, 취소, 진행률, 에러 진단을 한 곳에서 처리합니다. gix로 한 쓰기도 명령 로그에 남깁니다.
4. **명령 = 데이터 (M5)**: 모든 git 명령을 `CommandSpec`(이름, 옵션, 인자, 위험도)으로 기술해 폼·팔레트·단축키·커스텀 액션이 같은 스펙을 공유합니다.
5. **플랫폼 의존 코드는 한 곳에**: gf-app은 `src/platform/`, 하위 크레이트는 `gf_core::tool_path`. 피처 코드에는 `cfg!`가 없습니다.
6. **성능이 최우선**: 바뀐 것만 다시 읽고, 보이는 것만 그립니다(DEVELOPMENT.md §7).

---

## 3. 워크스페이스

크레이트 역할과 gf-app 내부 구조는 [DEVELOPMENT.md §2](docs/DEVELOPMENT.md)에 있습니다.
`gf-commands`(M5), `gf-flow`(M6), `gf-hosting`·`gf-hosting-gitlab`·`gf-hosting-github`·`gf-auth`(M7)는 빈 자리만 있습니다. 사용을 시작할 때 루트 `Cargo.toml`의 주석을 풉니다.

---

## 4. "모든 git 명령" 지원 전략 (3단 구조)

| 단계 | 대상 | 방식 | 상태 |
|---|---|---|---|
| 1. 전용 UI | 일상 명령(status, add, commit, branch, switch, merge, rebase, cherry-pick, stash, tag, fetch/pull/push, reset, revert, log, diff, blame, submodule, worktree, lfs, bisect, reflog 등) | 피처별 전용 화면 | 대부분 완료(M3·M4) |
| 2. 스펙 기반 자동 폼 | 나머지 서브커맨드(gc, fsck, archive, bundle, notes, rerere, sparse-checkout, maintenance, range-diff …) | `CommandSpec`으로 옵션 폼 생성 → 커맨드 팔레트에서 실행 | M5 |
| 3. 폴백 | 스펙에 없는 조합, alias, 플러그인 명령 | `gf ❯` 프롬프트에 직접 입력(지금 동작), 이후 내장 터미널 | 일부 완료 |

- 실행 결과는 항상 **명령 로그**에 실제 git 명령 그대로 남깁니다.
- 위험한 명령(reset --hard, push --force, clean -fd, branch -D 등)은 확인 다이얼로그를 거치고, 가능한 작업은 undo로 되돌릴 수 있게 합니다.

---

## 4-1. 디자인 방향: 터미널 스타일

앱 전체가 **터미널처럼 보이고 느껴지게** 만듭니다.

- **폰트**: 전 화면 고정폭 단일 폰트(macOS 시스템 고정폭, Windows Cascadia Mono → Consolas). 크기 단계는 최소로
- **색상**: 기본 다크, ANSI 16색 기반(추가=green, 삭제=red, 브랜치=cyan, 태그=yellow, HEAD=magenta). 프리셋(`gf-dark`, `gf-light`, `dracula`, `cd-dark`, `cd-light`)을 `:theme`으로 교체
- **레이아웃**: 둥근 모서리·그림자·그라디언트 없음. 패널 구분은 1px 선이나 박스 문자(`│ ─ ┌ ┐`) 느낌. 여백은 적게, 정보는 촘촘하게
- **커밋 그래프**: `● │ ├─ ╮ ╯` 같은 문자 기반
- **예외 — 파일 트리 폴더 아이콘**: 글자 한 칸 크기의 선 폴더(열림/닫힘)를 canvas로 그립니다. 글자처럼 테마 토큰(`t.folder()`) 색과 글꼴 크기를 따릅니다(`widgets/file_tree.rs`)
- **입력**: 하단 프롬프트(`gf ❯ `)가 기본 명령줄. 키보드 우선이지만 마우스도 1급(§4-2)
- **상태 표시**: 상태줄에 브랜치, ahead/behind, 진행 중 작업
- **버튼·다이얼로그**: `[ OK ]` `[ Cancel ]` 같은 텍스트 버튼, TUI 박스 스타일 다이얼로그
- 색·폰트 토큰은 `gf-app/src/theme/`에 모으고, 모든 위젯은 토큰만 참조합니다

## 4-2. 사용자 상호작용 원칙 (모든 UI 작업에 적용)

터미널 "느낌"이지만 사용성은 일반 GUI 이상이어야 합니다.

1. **마우스와 키보드 모두 1급**: 모든 동작은 클릭으로도, 단축키·명령줄로도 할 수 있습니다
2. **발견 가능성**: 단축키를 버튼 라벨·툴팁·상태줄에 함께 표시합니다(`[ Open ⌘O ]`, Windows는 `Ctrl+O`). `:help`로 전체 목록, 명령줄은 자동완성·히스토리(↑/↓)
3. **빈 상태를 안내로**: 저장소가 없으면 최근 저장소, 폴더 열기 버튼, 드래그&드롭 안내. 빈 목록에는 "무엇을 하면 채워지는지" 한 줄
4. **즉각적인 피드백**: 실행 중에는 스피너·진행률, 완료·실패는 상태줄과 명령 로그에 바로. 오류는 git 원문 + 다음 행동 제안
5. **안전장치(공통 확인 다이얼로그)**: 삭제·변경 작업은 반드시 `App::confirm(Confirm::…)`로 확인합니다(`gf-app/src/app/dialogs.rs`). 새 기능도 같은 헬퍼를 씁니다
   - `Confirm::delete`(빨강 ⚠): 데이터를 지우는 작업(discard, 브랜치·태그·stash 삭제, force push). Enter로 실행 불가, 클릭 필수, 건너뛰기 불가
   - `Confirm::change`(노랑 ?): 작업 트리·HEAD·원격을 바꾸는 작업(checkout, stash apply/pop, 통합이 필요한 pull, 태그 push, amend). 무엇이 바뀌는지와 되돌릴 수 있는지 적고, "다시 묻지 않기"를 고를 수 있음(`:confirm reset`으로 복원)
   - 잃을 것이 없을 때는 `Confirm::change(..).skip_if(..)`로 묻지 않고 바로 실행합니다(여전히 `App::confirm`을 거침). 예: 로컬 브랜치 checkout은 HEAD가 브랜치에 있고, merge·rebase·bisect 중이 아니고, 변경(ignored 제외)이 없으면 바로 전환(⌘Z로 복귀). 원격 브랜치 checkout은 로컬 브랜치를 새로 만드므로 항상 확인
   - 일반 Pull은 fast-forward만 바로 실행하고, 갈라졌거나 로컬 파일과 겹치면 확인 후 통합합니다. 일반 Push는 바로 실행하고, 거절 가능성이 알려졌거나 원격 ref를 강제로 바꾸는 설정이면 확인합니다
   - 확인 없이 실행: stage/unstage(hunk·줄 포함)처럼 자주 쓰고 바로 되돌릴 수 있는 작업, 사용자가 명령줄에 직접 입력한 git 명령
6. **포커스·상태 유지**: 탭 전환·새로고침 후에도 선택, 스크롤, 접힘 상태 유지. 재시작하면 열린 탭 복원
7. **큰 클릭 영역과 hover 표시**: 행 전체 클릭, hover 시 배경 강조. 촘촘해도 오클릭이 없게
8. **익숙한 관례**: 플랫폼 단축키(⌘O/⌘W/⌘1–9, Windows는 Ctrl), git 용어 그대로, 색 의미 일관(초록=추가, 빨강=삭제)
9. **폴더 드래그&드롭으로 저장소 열기**, 경로 입력에서 `~` 확장(Windows는 `C:\x`, `\\server\share`도)

---

## 5. 아직 없는 기능 후보

상용 툴(SourceTree, GitKraken, Fork, Tower, Sublime Merge, SmartGit)을 참고한 목록 중 아직 구현하지 않은 것입니다. 이미 있는 기능은 [README](README.md) "Why gf"에 있습니다.

- **보기**: 이미지 diff, 저장소 그룹(워크스페이스)
- **편집**: 커밋 템플릿·Conventional Commits 도우미·Co-author, 외부 머지툴 연동, 드래그&드롭으로 merge/rebase/cherry-pick
- **편의**: 커맨드 팔레트(⌘⇧P), 단축키 커스터마이즈, 자동 fetch와 원격 변경 배지, 커스텀 액션(사용자 스크립트를 메뉴·단축키에 등록), 한국어/영어 UI

---

## 6. Git Flow (M6)

- git-flow CLI 없이 순수 git 명령 조합으로 구현합니다.
- 설정은 `git config gitflow.*` 키를 **git-flow-avh와 같게** 저장해 터미널에서 git-flow를 쓰는 팀원과 호환합니다.
- 기능
  - `init`: master/develop 이름, prefix(feature/ release/ hotfix/ support/ bugfix/), 버전 태그 prefix
  - feature / bugfix / release / hotfix / support: start · finish · publish · track · delete
  - finish 옵션: squash, no-ff, keep branch, 태그 메시지, push 여부
  - 사이드바에 진행 중인 feature/release/hotfix 그룹
- 전략은 `trait BranchingModel`로 추상화해 GitHub Flow, trunk-based 등을 나중에 추가할 수 있게 합니다.

---

## 7. Git 서버 연동 (M7)

### 7.1 공통 trait
```rust
trait HostingProvider: RepoApi + MergeRequestApi + PipelineApi {
    fn kind(&self) -> ProviderKind;           // GitLab, GitHub, (추후) Bitbucket, Gitea
    fn matches_remote(&self, url: &Url) -> bool;
}
trait RepoApi        { list_repos, search_repos, get_repo, clone_urls(ssh/https) }
trait MergeRequestApi{ list, get, create, update, merge, close, approve/unapprove,
                       diffs, discussions/notes(라인 코멘트 포함), resolve_thread, assign_reviewers }
trait PipelineApi    { list_for_ref, get, jobs, retry, cancel }
```
- 지원하지 않는 기능은 `Unsupported`를 돌려주고, UI는 해당 메뉴를 비활성화합니다(Capabilities 플래그).

### 7.2 계정
- Personal Access Token(1차), OAuth Device Flow(2차)
- self-hosted GitLab: base URL, 사내 CA 인증서 옵션
- 토큰은 keyring에만 저장하고, 설정 파일에는 계정 메타데이터만 둡니다

### 7.3 Clone
- URL 입력 또는 연결된 계정의 저장소 검색 → 선택
- SSH/HTTPS, 대상 경로, 브랜치, depth, 재귀 submodule, LFS
- 진행률(`--progress`) 표시, 취소 가능
- HTTPS 토큰은 **GIT_ASKPASS 헬퍼**로 주입합니다(URL에 토큰을 넣지 않음)

### 7.4 GitLab MR (1차 완성 대상)
- 목록: 내가 만든 / 리뷰 요청 / 전체, 상태·라벨·작성자 필터
- 생성: 현재 브랜치 → 대상 브랜치, 템플릿, 리뷰어·담당자·라벨·마일스톤, draft, 머지 후 브랜치 삭제/squash
- 상세: 설명, 커밋, 변경 파일 diff(앱 diff 뷰 재사용), 파이프라인 상태
- 리뷰: 라인 코멘트, 스레드 답글/해결, approve
- 머지: merge / squash / rebase, 파이프라인 성공 시 머지
- MR 브랜치 로컬 체크아웃

### 7.5 GitHub
- 1차: 계정 연결, 저장소 목록, clone
- PR은 `MergeRequestApi` 구현만 추가하면 UI가 그대로 동작하게 설계합니다

---

## 8. 마일스톤

| 단계 | 목표 | 상태 |
|---|---|---|
| **M0 기반** | 워크스페이스, gf-core, tracing, 테스트 fixture, CI | ✅ |
| **M1 Git 코어** | Runner(취소·진행률·로그), porcelain 파서, gix 조회 | ✅ |
| **M2 앱 셸** | iced 창, Feature trait, 저장소 탭, 사이드바, 명령 로그, 테마 | ✅ |
| **M3 일상 작업** | hunk·줄 stage, 커밋, diff, 커밋 그래프, 브랜치/태그/stash, fetch/pull/push, 파일 감시 | ✅ |
| **M4 고급 작업** | merge/rebase/cherry-pick + 충돌 해결, interactive rebase, blame/파일 히스토리, undo, submodule/worktree/LFS, bisect, worktrees 패널 | ✅ |
| **M5 전 명령 지원** | CommandSpec, 자동 폼, 커맨드 팔레트, 내장 터미널, 커스텀 액션 | 다음 |
| **M6 Git Flow** | init 다이얼로그, start/finish/publish, Flow 사이드바 | |
| **M7 호스팅** | gf-auth, gf-hosting, GitLab(repo/MR/pipeline), Clone 다이얼로그, GitHub repo/clone | |
| **M8 macOS 배포** | 아키텍처별 .dmg, `install.sh`, 태그 기반 릴리스 워크플로 ✅ · 남음: 코드 서명·공증, 자동 업데이트, ko/en | 일부 |
| **M9 크로스플랫폼** | Windows x64(0.0.8부터, zip + `install.ps1`, CI 매트릭스) ✅ · 남음: Windows 실기 점검(한글 IME, DPI·다중 모니터), 코드 서명·MSI·winget, Linux | 일부 |

M6(Flow)과 M7(호스팅)은 서로 독립이라 병렬로 진행할 수 있습니다.

---

## 9. 테스트·품질

- **gf-git**: 임시 저장소(`gf-test-support::TestRepo`)로 시나리오 테스트(충돌, rebase, submodule, undo 등). 파서는 실제 git 출력으로 단위 테스트. gix 구현은 CLI와 결과를 비교하는 계약 테스트(`tests/gix_contract.rs`)
- **gf-app**: update 로직은 UI 없이 단위 테스트. 실제 App을 창 없이 조작하는 회귀 테스트와 UI 스윕(모든 버튼이 무언가를 하는지)
- **성능 기준**: 10만 커밋 저장소에서 그래프 첫 화면 < 500ms, status 갱신 < 200ms. 메모리는 `tools/mem-bench`로 측정
- **CI 게이트**: macOS·Windows에서 `cargo fmt --check`, `clippy -D warnings`, `cargo test`. `main`으로 가는 PR과 릴리스는 UI 스윕까지. 의존성 라이선스·취약점은 `cargo deny`

---

## 10. 결정이 필요한 항목

1. 앱 이름 / 번들 ID: 현재 가칭 `gf`(설정 경로 `dev.gf.gf`)
2. 라이선스: 오픈소스 여부(의존성 선택에 영향, `deny.toml`)
