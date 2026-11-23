# mem-bench — 메모리 측정 도구

실제 `App`을 창 없이 띄워 저장소 여러 개를 탭으로 열고, 단계마다 힙이 얼마나 남는지 잽니다. 탭을 여러 개 열었을 때의 메모리, 큰 diff, 깊은 히스토리 스크롤을 한 번에 봅니다.

- 측정 코드: [`bench.rs`](bench.rs). gf-app 테스트 바이너리에 `mem-bench` 기능을 켰을 때만 들어갑니다(`crates/gf-app/src/app/mod.rs`의 `mod mem_bench`). 평소 빌드·테스트에는 영향이 없습니다.
- 힙은 전역 할당자([`cap`](https://crates.io/crates/cap))로 셉니다. 운영체제와 관계없이 같은 값이 나옵니다.
- RSS는 [`memory-stats`](https://crates.io/crates/memory-stats)로 읽습니다(macOS · Windows · Linux).

## 실행

macOS · Linux 터미널, Windows의 Git Bash에서:

```bash
tools/mem-bench/run.sh                     # 샘플 저장소를 받아서(처음 한 번) 측정
tools/mem-bench/run.sh ~/src/a ~/src/b     # 내 저장소로 측정
GF_BENCH_DEEP=all tools/mem-bench/run.sh   # 첫 저장소 히스토리를 끝까지 스크롤
```

샘플 저장소는 `target/mem-bench/repos`에 전체 히스토리로 받습니다(약 530MB, 처음 한 번만).

| 저장소 | 커밋 | 보는 것 |
|---|---|---|
| `git/git` | 약 8만 | 그래프가 매우 넓은 저장소(행당 200 lane 이상) |
| `rust-lang/cargo` | 약 2.4만 | 큰 Rust 프로젝트 |
| `tokio-rs/tokio` | 약 5천 | 작은 프로젝트 |

마지막 탭은 측정 도구가 직접 만드는 합성 저장소입니다. 10MB 파일(30만 줄)을 통째로 바꾼 커밋이 있어서, 탭이 들고 있을 수 있는 diff의 최악 사례를 봅니다. 큰 diff는 ⏎를 누를 때까지 그리지 않으므로, 이 탭은 안내가 뜬 상태의 `view()`와 ⏎로 그린 뒤의 `view()`(`shown anyway`)를 따로 보여 줍니다.

환경 변수:

| 변수 | 기본값 | 뜻 |
|---|---|---|
| `GF_BENCH_DEEP` | `20000` | 첫 저장소 히스토리를 몇 커밋까지 스크롤할지. `all`이면 끝까지 |
| `GF_BENCH_DIR` | `target/mem-bench/repos` | 샘플 저장소를 받을 곳 |
| `GF_BENCH_CARGO` | (없음) | cargo에 더 줄 옵션. 예: `--release` |

스크립트 없이 직접 실행할 수도 있습니다. `GF_BENCH_REPOS`에 저장소 경로를 **한 줄에 하나씩** 넣습니다.

```bash
GF_BENCH_REPOS=$'/path/a\n/path/b' \
  cargo test -p gf-app --features mem-bench --bin gf mem_bench -- --ignored --nocapture --test-threads=1
```

PowerShell:

```powershell
$env:GF_BENCH_REPOS = "C:\src\a`nC:\src\b"
cargo test -p gf-app --features mem-bench --bin gf mem_bench -- --ignored --nocapture --test-threads=1
```

## 출력 읽기

```
--- tab 1: git
open tab (first history page)                Δheap     +3.9 MB     2446 ms | heap     5.4  high-water    25.9  rss   159.0
  history 1000 commits, view() first 0.45 ms, then 0.05 ms
select largest commit (26597 lines)          Δheap     +2.5 MB       42 ms | heap     7.2  high-water    25.9  rss   161.1
  if the commit diff were dropped            Δheap     -2.5 MB        1 ms | heap     4.7  high-water    25.9  rss   161.1
```

| 칸 | 뜻 |
|---|---|
| `Δheap` | 그 단계가 끝난 뒤 **남아 있는** 힙의 증감. 탭 하나, diff 하나의 비용입니다 |
| `ms` | 그 단계가 걸린 시간(git 실행, 백그라운드 작업 포함) |
| `heap` | 지금 살아 있는 힙 전체 |
| `high-water` | 프로세스 시작 뒤 한순간 가장 많이 쓴 힙. 로드 중 잠깐 두 벌을 들고 있으면 여기에 보입니다 |
| `rss` | 운영체제가 보는 실제 메모리. 할당자가 반납하지 않은 몫도 들어 있어서 `heap`보다 큽니다 |

단계:

1. **탭마다**
   - `open tab`: 저장소를 열고 첫 화면(히스토리 첫 페이지, status)을 읽은 비용
   - `select HEAD commit`, `select largest commit`: 커밋 diff를 열었을 때 탭이 들고 있는 양. 최근 2000개 커밋 중 가장 많이 바뀐 커밋을 고릅니다
   - `view()`: 화면 한 번을 다시 만드는 시간. iced는 메시지마다 다시 만듭니다. `first`는 새 데이터가 들어온 직후 첫 호출(Memo를 채우는 비용 포함), `then`은 그 뒤 평소 프레임입니다. 둘 다 커밋 수와 관계없이 일정해야 합니다
   - `if the commit diff were dropped`: 백그라운드 탭의 diff를 비우면 돌려받을 양
2. **history scroll**: 첫 저장소에서 "더 불러오기"를 반복합니다. 커밋 수에 따라 메모리와 `view()` 시간이 어떻게 늘어나는지, 그리고 히스토리 메모리가 어디에 쓰이는지(`breakdown`) 봅니다.
3. **close every tab**: 탭을 닫으면 메모리가 돌아오는지 봅니다. `left after closing`의 `heap`이 시작할 때와 비슷해야 합니다.

## 결과 비교하기

- 같은 기계, 같은 저장소, 같은 `GF_BENCH_DEEP`로 변경 전후를 잽니다.
- 시간은 기계와 디스크 캐시에 따라 흔들립니다. 첫 실행은 디스크 캐시가 비어 있어 느리니 두 번째 실행부터 비교합니다.
- `Δheap`은 할당자 기준이라 거의 흔들리지 않습니다.
- 기본 프로필은 의존성만 최적화합니다. 시간을 정확히 보려면 `GF_BENCH_CARGO=--release`를 씁니다(빌드가 오래 걸립니다).

참고 기록(Linux, 4코어, dev 프로필, `GF_BENCH_DEEP=all`, git.git 8만 커밋):

| 항목 | 처음 | 그래프 정리 후 | 페이지 이어 읽기·폭 미리 계산 후 |
|---|---|---|---|
| git.git 탭 열기 | 18.8MB | 3.9MB | 4.0MB |
| 8만 커밋 히스토리 | 1,090MB | 135MB | 117MB |
| 끝까지 스크롤하는 동안 high-water | 2.2GB | 262MB | 238MB |
| 8만 커밋까지 스크롤 총 시간 | 92초 | 49초 | 18초 |
| 페이지가 들어온 직후 첫 `view()` (8만 커밋) | — | 48ms | 0.6ms |
| 그 뒤 `view()` | 0.07ms | 0.07ms | 0.08ms |
