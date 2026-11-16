# gix 전환 계획

git 명령을 **영역별 인터페이스(trait)** 로 정의하고, 명령마다 **gix 구현** 또는 **git CLI 구현**을 골라 호출합니다.
gix가 지원하지 않거나 git과 동작이 달라지는 명령은 git CLI를 그대로 씁니다.

- 대상 버전: **gix 0.90** (2026-11 기준 최신). `0.x`라 마이너 버전마다 깨질 수 있으므로 `0.90` 계열로 고정하고, 올릴 때는 §5 계약 테스트를 통과해야 합니다.
- **stable 기능만** 사용합니다.
  - `unstable` 기능 플래그를 켜지 않습니다.
  - README 성숙도 등급에서 "안정화 후보"(gix-ref, gix-config, gix-hash, gix-actor)와 "usable"(gix-object, gix-diff, gix-status, gix-worktree 등)만 씁니다.
  - "very early"(gix-blame)와 "idea"(gix-rebase, gix-sequencer, gix-lfs)는 쓰지 않습니다.
- gix는 아직 **hook을 실행하지 않고**, **GPG 서명**도 지원하지 않습니다. 이 둘이 걸리는 쓰기(commit, checkout, push 등)는 CLI에 남깁니다.

## 1. 구조

```
gf-git/src/
├─ backend/
│  ├─ mod.rs          영역별 trait(인터페이스) — BranchOps, LogOps, StatusOps …
│  ├─ cli.rs          CliBackend: 시스템 git 실행(command/read/exec 헬퍼)
│  ├─ gix_backend.rs  GixBackend: gix 조회 구현(log·ref·reflog·stash·worktree·rebase). 지원하지 않는 메서드는 Err(Unsupported)
│  ├─ gix_status.rs   GixBackend: status, numstat
│  ├─ gix_refs.rs     GixBackend: 브랜치·경량 태그 ref 편집
│  └─ gix_fetch.rs    GixBackend: fetch --prune (기능 `gix-fetch`, 설정으로 켤 때만)
├─ ops/*.rs      영역별 타입·파서와 `impl XxxOps for CliBackend` (CLI 구현)
├─ routes.rs     Repository → 명령별 백엔드 선택표(이 파일만 보면 누가 처리하는지 압니다)
└─ repository.rs Repository: UI가 쓰는 진입점(open, handle, run …)
```

- **trait 정의:** ops 파일 하나가 trait 하나입니다. 메서드는 `fn x(&self, …) -> impl Future<Output = Result<T>> + Send`(RPITIT) 형태입니다.
  - dyn이 아니라 정적 디스패치이고, iced `Task::perform`이 요구하는 `Send`를 보장합니다.
  - 구현에서는 `async fn`으로 씁니다.
- **`Repository`는 공개 메서드 이름을 그대로 유지합니다.** 그래서 gf-app은 바뀌지 않습니다.
  - `routes.rs`에서 각 메서드가 어느 백엔드를 부를지 정합니다.
  - gix 우선 명령은 `gix_then_cli!(self, XxxOps::x(…))`로 씁니다. gix가 `Unsupported`를 돌려주거나 실패하면 CLI로 다시 시도합니다(실패는 `tracing::warn`).
  - GixBackend는 저장소 `config` 파일이 바뀌면 gix 핸들을 다시 엽니다. 그래서 CLI로 바꾼 remote·upstream 설정이 바로 반영됩니다.
  - 판단이 애매한 경우(URL 재작성 `insteadOf`, 서명된 태그, 여러 URL 등)는 결과가 달라질 위험을 지지 않고 `Unsupported`로 CLI에 넘깁니다.
- **명령 로그:** gix로 실행한 쓰기도 `GitRunner::report_in_process`로 명령 로그에 남깁니다(예: `gix branch feature/x`, 출력 `Created branch feature/x at abc1234.`).
- **에러:** gix 에러는 `GfError`의 별도 변형으로 구분합니다. 같은 실패를 CLI로 재시도했을 때 나오는 stderr로 실패 다이얼로그(`diagnose`)가 분류합니다.
- **끄는 방법:** 환경변수 `GF_GIT_BACKEND=cli` 또는 Cargo 기능 `gix` 끄기(`--no-default-features`)로 언제든 CLI만 쓸 수 있습니다.
- **gix fetch 켜기:** 기본은 꺼져 있습니다. 프롬프트에서 `:gix-fetch on`(설정 `git.gix_fetch = true`), 끄려면 `:gix-fetch off`.

## 2. 명령별 백엔드

| 영역(trait) | 명령 | 백엔드 | 근거 |
|---|---|---|---|
| LogOps | log: 기본 순서, `--first-parent`, 범위(`a..b`, `^a`), `--all`, skip/max-count | **gix** ✅ | |
| | log: `--topo-order`·`--date-order` | **gix** ✅ (commit-graph가 있을 때만) | commit-graph가 있으면 gix가 빠르고(3배), 없으면 git이 3배 빠릅니다. shallow 저장소는 CLI |
| | log: 경로 필터, `--follow`, `--author`, `--grep`, `-S`, `a...b` | CLI | gix가 `Unsupported` 반환 |
| | head_exists | **gix** ✅ | |
| RefOps / RemoteOps | branches(upstream·ahead/behind·gone 포함), tags, remotes, unpushed_count, unpushed | **gix** ✅ | gix-ref·config 안정화 후보. 서명된 태그, `insteadOf`, URL이 여러 개인 remote는 CLI |
| ReflogOps | reflog, head_oid | **gix** ✅ | |
| | reset, undo_to | CLI | 고수준 reset 없음 |
| StashOps | stashes(목록) | **gix** ✅ | `refs/stash` reflog |
| | push/apply/pop/drop | CLI | |
| WorktreeOps | 목록 | **gix** ✅ | unborn 브랜치의 worktree는 CLI |
| | add/remove/prune | CLI | |
| SubmoduleOps | 목록(상태 포함), update/sync | CLI | 목록의 `(describe)`가 git의 describe 단계(태그 → `--tags` → `--contains` → `--all`)와 같아야 해서 CLI 유지 |
| RebaseOps | rebase_plan, squash_preview | **gix** ✅ | 두 백엔드가 같은 로직(`rebase_interactive::plan/preview`)을 공유하고 log·unpushed만 각자 구현 |
| | rebase_interactive, squash | CLI | gix-sequencer 없음 |
| StateOps | state | 파일 검사(백엔드 무관) | |
| StatusOps | status | **gix** ✅ | 3~7배. submodule은 `--ignore-submodules=none`과 같게(새 커밋·수정·untracked → 수정됨). intent-to-add 항목은 CLI |
| DiffOps | numstat | **gix** ✅ | 4~10배. gix diff 파이프라인(필터·바이너리 판정 git과 동일). submodule은 git 규칙대로(다른 커밋 `1 1`, 같은 커밋의 수정 `0 0`, untracked만 있으면 제외). 외부 diff 드라이버, 충돌 중 경로는 CLI |
| | diff(hunk) | CLI | hunk 위치가 git xdiff 휴리스틱과 정확히 같아야 하고, 파일 하나를 클릭할 때 한 번 실행되는 명령이라 이득이 작습니다 |
| BlameOps | blame | CLI | gix-blame "very early" |
| BranchOps | create(체크아웃 없이), delete | **gix** ✅ | ref 트랜잭션. reflog 메시지까지 git과 동일(`branch: Created from main`). 아래 경우는 CLI |
| | ↳ CLI로 보내는 경우 | CLI | `reference-transaction` hook 있음, remote 추적 브랜치에서 생성(git이 upstream 설정), `branch.autoSetupMerge`가 기본값 아님, 삭제할 브랜치에 `branch.<name>.*` 설정 있음(git이 함께 지움), 어느 worktree에서든 사용 중, `-d`인데 병합 안 됨 |
| | rename, switch, checkout_detached, delete_remote_branch, check_branch_name | CLI | reflog·설정 이동, worktree 갱신, push |
| TagOps | 경량 태그 생성·삭제 | **gix** ✅ | `git tag`처럼 대상 객체를 벗기지 않고 가리킴(태그의 태그) |
| | 주석 태그, push_tag | CLI | |
| CommitOps | commit, head_message | CLI | hook, 서명 |
| IndexOps / ConflictOps | stage, unstage, discard, apply_patch, 충돌 해결 | CLI | gix-apply 미구현 |
| IntegrateOps | merge, rebase, cherry-pick, revert, continue/skip/abort | CLI | sequencer 없음 |
| SyncOps | fetch(`--prune`, 한 remote 또는 `--all`) | **gix** ✅ (설정을 켰을 때만, 기본 CLI) | 진행률·취소·명령 로그 지원. 아래 차이 참고 |
| | ↳ CLI로 보내는 경우 | CLI | shallow 저장소, submodule 있음(git은 필요 시 submodule도 fetch), `src/*:dst/*` 외 refspec, `--all`에서 제외 설정된 remote, 등록되지 않은 URL, `followRemoteHEAD=always`, gix 오류(인증 실패 등) |
| | pull, push | CLI | 고수준 push 없음 |
| BisectOps / LfsOps | 전부 | CLI | 미구현 |
| (Repository::run) | 프롬프트의 임의 git 명령 | CLI | "모든 git 명령" 경로 |

## 3. 단계

| 단계 | 내용 | 완료 기준 |
|---|---|---|
| **P0** 인터페이스 분리 ✅ | `backend/` trait와 `CliBackend`를 만들고, ops 구현을 `impl XxxOps for CliBackend`로 옮깁니다. `routes.rs`가 전부 CLI로 보냅니다(log는 기존 gix 경로 유지) | 동작 변화 없음, 기존 테스트 전부 통과 |
| **P1** gix 조회 A ✅ | refs, remotes, reflog, stash 목록, worktree 목록, head, topo log, unpushed, rebase_plan, squash_preview | `tests/gix_contract.rs` 계약 테스트 통과(ahead/behind/gone upstream, 로컬 upstream, 경량·주석 태그, 여러 remote, stash, 잠긴 worktree, detached·unborn HEAD, 설정 변경 반영) |
| **P2** gix 조회 B ✅ | status, numstat (diff hunk는 CLI 유지) | 계약 테스트 통과(스테이징·워킹트리·이름변경+수정·권한 변경·줄끝 개행 변경·바이너리·중첩 untracked·충돌·detached·upstream), 벤치마크에서 이득 확인 |
| **P2b** submodule ✅ | submodule이 있는 저장소의 status·numstat (submodule 목록은 CLI 유지) | 계약 테스트 통과(깨끗함·다른 커밋·내용 수정·untracked·스테이징된 새 커밋·새로 추가·미초기화), 실제 저장소 3곳에서 벤치마크 결과 일치 |
| **P3** gix ref 쓰기 ✅ | 브랜치 생성·삭제, 경량 태그 생성·삭제 (이름변경은 reflog·설정 이동 때문에 CLI 유지) | 같은 저장소 두 벌에 CLI와 gix로 각각 적용해 ref·reflog·설정이 같은지 비교하는 계약 테스트 8건 통과, git이 해야 하는 경우는 CLI로 넘어가는지 확인, 명령 로그 기록 확인 |
| **P4** 선택 기능 ✅ | fetch를 gix로(설정 `git.gix_fetch`, `:gix-fetch on`) | 같은 원격 변경을 두 clone에 CLI·gix로 각각 fetch해 ref가 같은지 비교(새 커밋·새 브랜치·prune·강제 갱신·태그 자동 추적·도달 불가 태그 제외·remote HEAD 생성·`--all`), 객체 연결성 검사, 취소·대체 경로 테스트. 실제 HTTPS·SSH 원격과 인증 도우미로는 아직 확인하지 않음 |
| 보류 | commit, checkout, reset, merge 계열, stash 쓰기, push/pull, blame, bisect, LFS | gix의 해당 crate가 "usable" 이상이 되면 다시 검토 |

### gix fetch가 git과 다른 점 (P4)

- `--prune`은 gix에 없어서 직접 구현했습니다: fetch 후 각 refspec 대상(`refs/remotes/<name>/`) 아래에서 원격에 더 이상 없는 ref를 ref 트랜잭션으로 지웁니다.
- git 2.48+처럼 없는 `refs/remotes/<name>/HEAD`를 원격 HEAD로 만듭니다. 설치된 git이 2.48 미만이면 만들지 않습니다(같은 결과를 위해 `git --version`을 한 번 확인).
- `FETCH_HEAD`를 쓰지 않고, `gc --auto`를 실행하지 않으며, remote 추적 ref의 reflog 메시지가 git과 다릅니다. pull은 CLI가 다시 fetch하므로 영향이 없습니다.
- HTTPS는 시스템 TLS(macOS Security.framework), SSH는 `ssh` 프로그램, 자격 증명은 git과 같은 credential helper를 씁니다. 실패하면 CLI가 다시 fetch합니다.

## 4. 새 명령을 추가할 때

1. 해당 영역 trait(`backend/mod.rs`)에 메서드를 추가합니다.
2. `ops/<영역>.rs`의 `impl XxxOps for CliBackend`에 CLI 구현을 추가합니다. 모든 명령은 CLI 구현을 반드시 가집니다.
3. gix로 처리할 수 있으면 GixBackend에도 구현하고 §5 계약 테스트를 추가합니다.
4. `routes.rs`에 `Repository` 메서드를 추가하고 백엔드를 고릅니다. 위 §2 표도 갱신합니다.

## 5. 검증

### 벤치마크 (P2b 시점, `tests/gix_bench.rs`, 5회 중 최소값)

벤치마크는 두 백엔드의 결과가 다르면 `MISMATCH`를 출력합니다. 아래 저장소에서는 모두 일치했습니다.

```bash
GF_BENCH_REPO=/path/to/repo cargo test -p gf-git --release --test gix_bench -- --ignored --nocapture
```

| 명령 | 4만 커밋·submodule 있음 | oh-my-zsh(shallow) | gf |
|---|---|---|---|
| status | 44.4 → 13.5ms | 14.8 → 4.9ms | 8.5 → 1.2ms |
| numstat(unstaged) | 28.9 → 5.5ms | 8.9 → 1.3ms | 13.8 → 3.2ms |
| log topo 1000 | 16.7 → 5.6ms | CLI (shallow) | CLI (commit-graph 없음) |
| branches | 8.0 → 1.1ms | 7.9 → 0.18ms | 6.7 → 0.04ms |
| unpushed_count | 13.1 → 0.5ms | 13.5 → 1.6ms | 13.2 → 0.17ms |
| stashes | 5.9 → 0.01ms | 6.0 → 0.01ms | 6.2 → 0.02ms |

- 계약 테스트는 `crates/gf-git/tests/gix_contract.rs`에 있습니다. 새 gix 구현을 추가하면 여기에 `same(&repo, "이름", |b| async move { call!(b, XxxOps::x(…)) })` 한 줄을 추가합니다.

- **계약 테스트:** 같은 `TestRepo` fixture에서 gix와 CLI 결과가 같은지 비교합니다(`assert_eq!(gix, cli)`). fixture는 다음 경우를 포함합니다.
  - merge가 있는 저장소, 이름 변경, 바이너리 파일
  - submodule, worktree, detached HEAD, unborn 브랜치
  - 충돌 상태, 비ASCII 경로
- **CI:** 기본 빌드와 `--no-default-features`(CLI만) 둘 다 테스트합니다.
- **벤치마크:** 큰 저장소에서 비교해 **더 빠르고 결과가 같을 때만** gix로 바꿉니다.
- **캐시 무효화:** 파일 감시기가 `.git/config`나 packed-refs 변경을 알리면 gix 핸들을 다시 엽니다.

참고: [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md), [STABILITY.md](https://github.com/GitoxideLabs/gitoxide/blob/main/STABILITY.md)
