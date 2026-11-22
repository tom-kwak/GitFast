# gf — 에이전트 안내

Rust + iced 0.14 Git GUI 클라이언트입니다(macOS · Windows).

작업 전에 반드시 읽을 문서:
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): 피처 개발 지침(구조, 템플릿, UI·키보드 규칙, 완료 기준)
- [PLAN.md](PLAN.md): 제품 방향과 마일스톤

필수 규칙 요약:
- 완료 기준: `cargo fmt` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace`
- 커밋은 사용자가 요청할 때만 합니다.
- 삭제·변경 작업은 공통 확인 다이얼로그(`App::confirm`)를 거칩니다.
- 색은 테마 토큰으로만 지정합니다.
- 성능 규칙(DEVELOPMENT.md §7)을 지킵니다: 새로고침은 `request_parts`/`request_touched`로 요청만 하고, `view()`에서 목록 전체를 훑지 않으며, UI 스레드에서 git이나 큰 파일 I/O를 하지 않습니다.
- 운영체제별 코드는 gf-app 안에서는 `gf-app/src/platform/`, 하위 크레이트에서는 `gf_core::tool_path`에 둡니다. 피처 코드에서 `cfg!`를 쓰지 않습니다(DEVELOPMENT.md §4-1).
- 툴체인 경로: macOS `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`, Windows는 rustup 기본 경로(`%USERPROFILE%\.cargo\bin`, Git Bash에서는 `export PATH="$HOME/.cargo/bin:$PATH"`)
