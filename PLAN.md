# gf — Rust Git GUI Client 개발 계획

> 대상: macOS 네이티브 앱(.app / .dmg) 우선, 이후 Windows·Linux 확장
> 언어: Rust (전 계층)

---

## 1. 핵심 기술 결정

| 영역 | 선택 | 이유 |
|---|---|---|
| UI 프레임워크 | **iced** (권장) / 대안: gpui | 순수 Rust, macOS·Windows·Linux 공통 지원, Elm 아키텍처(State/Message/update/view)라 기능을 모듈 단위로 나누기 좋음. 커밋 그래프·diff 같은 커스텀 위젯은 `canvas`/custom widget으로 구현 |
| Git 쓰기·실행 | **시스템 `git` CLI 호출** | "모든 git 명령어 지원"을 만족하는 유일한 방법. hooks, credential helper, LFS, GPG/SSH signing, `.gitconfig`가 사용자의 터미널과 100% 동일하게 동작 |
| Git 읽기(고속) | **gix (gitoxide)** | 대형 저장소에서 log/graph/status/blame 조회를 프로세스 스폰 없이 빠르게 처리. 실패 시 CLI로 폴백 |
| 비동기 | tokio | git 프로세스, HTTP, 파일 감시를 UI 스레드와 분리 |
| HTTP | reqwest (rustls) | GitLab/GitHub API |
| 자격증명 저장 | keyring | macOS Keychain / Windows Credential Manager / Linux Secret Service 공통 추상화 |
| 파일 감시 | notify | 작업 트리 변경 시 status 자동 갱신 |
| 설정 저장 | serde + toml, directories | OS별 표준 설정 경로 |
| Diff 하이라이트 | syntect | 언어별 구문 강조 |
| 내장 터미널 | alacritty_terminal (+ 자체 렌더) | 구조화 UI로 표현 못 하는 명령의 최종 폴백 |
| 패키징 | cargo-packager | .app/.dmg 생성, 이후 .msi/.deb/AppImage까지 동일 도구로 확장 |
| 로깅 | tracing | 실행된 git 명령/출력 전부를 "명령 로그" 패널에 표시 |

> 사전 준비: 현재 머신에 Rust 툴체인이 없음 → `rustup` 설치 필요.

---

## 2. 아키텍처 원칙

1. **UI와 코어 완전 분리** — 코어 crate들은 UI 의존성 0. UI 프레임워크 교체(iced→gpui)나 CLI/테스트 하네스가 코어를 그대로 재사용.
2. **trait 기반 확장 지점** — Git 백엔드, 호스팅 프로바이더, 기능(Feature) 모듈을 모두 trait으로 정의하고 레지스트리에 등록. 새 기능 = 새 파일/crate 추가 + 등록 한 줄.
3. **명령 = 데이터** — 모든 git 명령은 `CommandSpec`(이름, 옵션, 인자 스키마, 위험도)으로 기술. UI 폼·커맨드 팔레트·단축키·커스텀 액션이 같은 스펙을 공유.
4. **모든 실행은 하나의 Runner를 통과** — 로그, 취소, 진행률, 에러 파싱, 위험 명령 확인, Undo 스냅샷을 한 곳에서 처리.
5. **플랫폼 의존 코드는 `platform` 모듈에 격리** — 메뉴바, Keychain, Finder/에디터 열기 등. `cfg(target_os)` 분기는 여기에만.

---

## 3. Cargo 워크스페이스 구조

```
gf/
├─ Cargo.toml                    # workspace
├─ crates/
│  ├─ gf-core/                   # 공통 도메인: 모델(Commit, Branch, Remote, FileStatus…), 에러, 이벤트 버스, ID
│  ├─ gf-git/                    # Git 백엔드
│  │  ├─ src/backend.rs          #   trait GitReader / GitWriter
│  │  ├─ src/cli/                #   CLI 실행기: runner.rs, progress.rs, env.rs(askpass/credential)
│  │  │  └─ parse/               #   porcelain 파서: status_v2.rs, log.rs, diff.rs, branch.rs, stash.rs, blame.rs …
│  │  ├─ src/gix/                #   gitoxide 기반 고속 조회
│  │  └─ src/ops/                #   고수준 연산 (파일 단위 분리)
│  │     ├─ status.rs  commit.rs  branch.rs  merge.rs  rebase.rs  cherry_pick.rs
│  │     ├─ stash.rs   tag.rs     remote.rs  fetch_pull_push.rs  reset_revert.rs
│  │     ├─ submodule.rs  worktree.rs  lfs.rs  bisect.rs  blame.rs  reflog.rs  clean.rs …
│  ├─ gf-commands/               # 명령 레지스트리
│  │  ├─ src/spec.rs             #   CommandSpec / OptionSpec / ArgSpec / RiskLevel
│  │  ├─ src/registry.rs         #   등록·검색 (팔레트용 퍼지 검색)
│  │  └─ specs/*.toml            #   git 전체 서브커맨드 스펙 (git help -a 기반, 데이터 파일로 관리)
│  ├─ gf-flow/                   # Git Flow
│  │  ├─ src/config.rs           #   gitflow.* 설정 (git-flow-avh 호환 키)
│  │  ├─ src/init.rs             #   초기화
│  │  └─ src/{feature,release,hotfix,support,bugfix}.rs   # start/finish/publish/track/delete
│  ├─ gf-hosting/                # 호스팅 추상화
│  │  ├─ src/provider.rs         #   trait HostingProvider (+ 하위 trait: RepoApi, MergeRequestApi, PipelineApi)
│  │  ├─ src/model.rs            #   공통 모델: RemoteRepo, MergeRequest, Note, Discussion, Approval, Pipeline
│  │  └─ src/registry.rs         #   remote URL → provider 자동 매칭
│  ├─ gf-hosting-gitlab/         # GitLab 구현 (MR 포함, self-hosted base URL 지원)
│  ├─ gf-hosting-github/         # GitHub 구현 (1차: clone/repo 목록, PR은 trait만 맞춰 확장 예정)
│  ├─ gf-auth/                   # 계정·토큰(keyring), OAuth(추후), SSH 키 관리, git credential helper 연동
│  ├─ gf-config/                 # 앱 설정, 저장소 북마크/워크스페이스, 단축키, 테마
│  ├─ gf-watch/                  # 파일 감시 → 이벤트 버스
│  └─ gf-app/                    # iced 앱 (바이너리)
│     ├─ src/main.rs
│     ├─ src/app.rs              #   루트 State/Message 라우팅
│     ├─ src/feature.rs          #   trait Feature { id, menu_items, commands, view, update, subscriptions }
│     ├─ src/features/           #   기능별 모듈 (하나의 폴더 = 하나의 기능)
│     │  ├─ repo_tabs/  sidebar/  history_graph/  changes/  diff/  commit/
│     │  ├─ branches/   stash/    tags/  remotes/  submodules/  worktrees/
│     │  ├─ merge_conflict/  interactive_rebase/  blame/  file_history/  reflog_undo/
│     │  ├─ git_flow/   clone/    hosting_accounts/  merge_requests/
│     │  ├─ command_palette/  command_runner/  terminal/  command_log/  settings/
│     ├─ src/widgets/            #   재사용 위젯: commit_graph, diff_view, virtual_list, split_pane, toast
│     ├─ src/theme/              #   라이트/다크, 색상 토큰
│     ├─ src/i18n/               #   ko / en (fluent)
│     └─ src/platform/           #   macos.rs / windows.rs / linux.rs
├─ tests/fixtures/               # 테스트용 저장소 생성 스크립트
└─ packaging/                    # 아이콘, Info.plist, 서명/공증 설정
```

---

## 4. "모든 git 명령어" 지원 전략 (3단 구조)

| 단계 | 대상 | 방식 |
|---|---|---|
| 1. 전용 UI | 일상 명령 (status, add, commit, branch, checkout/switch, merge, rebase, cherry-pick, stash, tag, fetch/pull/push, reset, revert, log, diff, blame, submodule, worktree, lfs, bisect, clean, reflog 등) | 기능 모듈별 전용 화면, 드래그&드롭 |
| 2. 스펙 기반 자동 폼 | 그 외 모든 서브커맨드 (`git help -a` 전체: gc, fsck, archive, bundle, notes, rerere, sparse-checkout, maintenance, filter-branch, replace, range-diff …) | `specs/*.toml`의 CommandSpec으로 옵션 폼 자동 생성 → 커맨드 팔레트에서 실행 |
| 3. 폴백 | 스펙에 없는 플래그 조합, 외부 alias, 플러그인 명령 | 내장 터미널 / 직접 명령 입력 (자동완성 지원) |

- 실행 결과는 항상 **명령 로그 패널**에 실제 git 명령 그대로 노출 → 학습/디버깅에 유용.
- `RiskLevel::Destructive`(reset --hard, push --force, clean -fd, branch -D 등)는 확인 다이얼로그 + 실행 전 자동 백업 ref 생성.

---

## 5. 상용 툴에서 가져올 기능 (SourceTree / GitKraken / Fork / Tower / Sublime Merge / SmartGit 참고)

**보기·탐색**
- 커밋 그래프 (레인 색상, 브랜치/태그 배지, 가상 스크롤로 10만+ 커밋 대응)
- 커밋 검색 (메시지, 작성자, SHA, 파일 경로, `-S` pickaxe 내용 검색)
- 파일 히스토리, Blame (커밋 단위 이동), 이미지 diff
- diff: side-by-side / inline, 단어 단위 하이라이트, 공백 무시, 구문 강조
- 멀티 저장소 탭, 저장소 그룹(워크스페이스), 즐겨찾기

**편집·작업**
- Hunk / 라인 단위 stage·unstage·discard
- 커밋: amend, 서명(GPG/SSH), 커밋 템플릿, Conventional Commits 도우미, Co-author
- 인터랙티브 리베이스 GUI (드래그로 순서 변경, squash/fixup/reword/drop)
- 드래그&드롭: 브랜치→브랜치(merge/rebase), 커밋→브랜치(cherry-pick)
- 3-way 머지 충돌 해결 에디터 (ours/theirs/both, 블록 단위 선택) + 외부 머지툴 연동
- Stash: 부분 stash, 미리보기, apply/pop/branch
- **Undo/Redo** (reflog 기반, 최근 작업 되돌리기)
- Bisect 마법사, Submodule/Worktree/LFS 관리 화면

**편의**
- 커맨드 팔레트 (⌘⇧P), 전역 단축키 커스터마이즈
- 자동 fetch(주기 설정), 원격 변경 알림 배지
- 커스텀 액션 (사용자 정의 스크립트/명령을 메뉴·단축키에 등록)
- 외부 에디터/터미널/Finder 열기
- 다크/라이트 테마, 한국어/영어

---

## 6. Git Flow

- **CLI(git-flow-avh) 설치 불필요** — 순수 git 명령 조합으로 자체 구현.
- 설정은 `git config gitflow.*` 키를 **git-flow-avh와 동일하게** 저장 → 터미널에서 git-flow를 쓰는 팀원과 상호 호환.
- 기능
  - `init`: master/develop 브랜치명, prefix(feature/ release/ hotfix/ support/ bugfix/), version tag prefix 설정 다이얼로그
  - feature / bugfix / release / hotfix / support: start · finish · publish · track · delete
  - finish 옵션: squash, no-ff, keep branch, 태그 메시지, push 여부
  - 사이드바에 Flow 전용 섹션 (진행 중인 feature/release/hotfix 그룹 표시)
- 확장성: 전략을 `trait BranchingModel`로 추상화 → 이후 GitHub Flow, GitLab Flow, trunk-based 템플릿 추가 가능.

---

## 7. Git 서버 연동

### 7.1 공통 trait
```rust
trait HostingProvider: RepoApi + MergeRequestApi + PipelineApi {
    fn kind(&self) -> ProviderKind;           // GitLab, GitHub, (추후) Bitbucket, Gitea
    fn matches_remote(&self, url: &Url) -> bool;
}
trait RepoApi        { list_repos, search_repos, get_repo, clone_urls(ssh/https) }
trait MergeRequestApi{ list, get, create, update, merge, close, approve/unapprove,
                       diffs, discussions/notes(라인 코멘트 포함), resolve_thread, assign_reviewers }
trait PipelineApi    { list_for_ref, get, jobs, retry, cancel }   // MR 화면 상태 표시용
```
- 기능 미지원 프로바이더는 `Unsupported` 에러 반환 → UI에서 해당 메뉴 자동 비활성화(Capabilities 플래그).

### 7.2 계정
- Personal Access Token 등록 (1차), OAuth Device Flow (2차)
- self-hosted GitLab: base URL 입력, 사내 CA 인증서 옵션
- 토큰은 keyring(Keychain)에만 저장, 설정 파일에는 계정 메타데이터만

### 7.3 Clone
- URL 직접 입력 또는 연결된 계정의 저장소 목록 검색 → 선택
- SSH/HTTPS 선택, 대상 경로, 브랜치, depth(shallow), 재귀 submodule, LFS
- 진행률(`--progress` 파싱) 표시, 취소 가능
- HTTPS clone 시 저장된 토큰을 **GIT_ASKPASS 헬퍼**로 안전하게 주입 (URL에 토큰 박지 않음)

### 7.4 GitLab MR (1차 완성 대상)
- 목록: 내가 만든 / 리뷰 요청 / 전체, 상태·라벨·작성자 필터
- 생성: 현재 브랜치 → 대상 브랜치, 제목/설명 템플릿, 리뷰어·담당자·라벨·마일스톤, draft, 머지 후 브랜치 삭제/squash 옵션
- 상세: 설명, 커밋, 변경 파일 diff(앱 diff 뷰 재사용), 파이프라인 상태
- 리뷰: 라인 코멘트, 스레드 답글/해결, approve
- 머지: merge / squash / rebase 옵션, 파이프라인 성공 시 머지
- MR 브랜치 로컬 체크아웃

### 7.5 GitHub
- 1차: 계정 연결, 저장소 목록, clone
- PR 기능은 `MergeRequestApi` 구현만 추가하면 UI가 그대로 동작하도록 설계 (사용자 검증 후 확장)

---

## 8. 단계별 마일스톤

| 단계 | 목표 | 주요 산출물 |
|---|---|---|
| **M0 기반** | 워크스페이스·CI 골격 | rustup, workspace, gf-core, tracing, 테스트 fixture, GitHub Actions(macOS) |
| **M1 Git 코어** | CLI Runner + 파서 | 취소/진행률/로그 Runner, status v2·log·diff·branch 파서, gix 조회, 단위테스트 |
| **M2 앱 셸** | 빈 앱 실행 | iced 윈도우, Feature trait, 저장소 열기/탭, 사이드바, 명령 로그 패널, 테마 |
| **M3 일상 작업** | SourceTree 수준 기본기 | 변경사항/hunk·라인 stage, 커밋, diff 뷰, 커밋 그래프, 브랜치/태그/stash, fetch/pull/push, 파일 감시 |
| **M4 고급 작업** | 상용툴 차별 기능 | merge/rebase/cherry-pick + 충돌 해결, 인터랙티브 리베이스, blame/파일 히스토리, Undo, submodule/worktree/LFS, bisect |
| **M5 전 명령 지원** | "모든 명령" 달성 | CommandSpec 전체 작성, 자동 폼, 커맨드 팔레트, 내장 터미널, 커스텀 액션 |
| **M6 Git Flow** | Flow 지원 | init 다이얼로그, start/finish/publish, Flow 사이드바 |
| **M7 호스팅** | Clone + GitLab MR | gf-auth, gf-hosting, GitLab 구현(repo/MR/pipeline), Clone 다이얼로그, MR 화면, GitHub repo/clone |
| **M8 배포** | macOS 배포 | cargo-packager .app/.dmg, 코드서명·공증, 자동 업데이트, ko/en |
| **M9 크로스플랫폼** | Windows/Linux | platform 모듈 구현, CI 매트릭스, 설치 패키지 |

각 단계는 이전 단계에만 의존하며, M6(Flow)과 M7(호스팅)은 M3 완료 후 병렬 진행 가능.

---

## 9. 테스트·품질

- **gf-git**: 임시 디렉터리에 실제 git 저장소를 만들어 시나리오 테스트 (충돌, rebase, submodule 등). 파서는 실제 git 출력 스냅샷 테스트(insta).
- **gf-flow**: 시나리오별 브랜치/태그 결과 검증, git-flow-avh와 설정 키 호환성 테스트.
- **gf-hosting-***: wiremock으로 API 모킹, 실 GitLab 대상 통합 테스트는 opt-in(환경변수 토큰).
- **gf-app**: update 로직(State+Message)은 UI 없이 단위 테스트 가능 — Elm 구조의 장점.
- 성능 기준: 10만 커밋 저장소에서 그래프 첫 화면 < 500ms, status 갱신 < 200ms.
- `cargo clippy -D warnings`, `cargo fmt`, `cargo deny`(라이선스/취약점) CI 게이트.

---

## 10. 결정이 필요한 항목

1. **UI 프레임워크**: iced(권장, 안정적·문서 풍부·크로스플랫폼) vs gpui(Zed 엔진, 최고 성능·macOS 최적이나 API가 불안정하고 문서 부족)
2. 앱 이름 / 번들 ID (현재 가칭 `gf`)
3. 라이선스 (오픈소스 여부 → 의존성 선택에 영향)
4. 첫 배포 최소 macOS 버전 (권장: 13 Ventura 이상)
