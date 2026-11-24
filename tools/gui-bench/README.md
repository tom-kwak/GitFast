# gui-bench — 다른 Git GUI와 메모리 비교 (macOS)

이미 실행 중인 GUI 앱 하나가 **한 가지 동작을 하는 동안** 쓰는 메모리와 걸린 시간을 잽니다. gf와 다른 Git GUI(GitHub Desktop, Fork, Sourcetree 등)를 같은 조건으로 비교할 때 씁니다.

[tools/mem-bench](../mem-bench/README.md)와 다른 점:

| | mem-bench | gui-bench |
|---|---|---|
| 대상 | gf만 (창 없이 `App`을 띄움) | 실제로 띄운 아무 macOS 앱 |
| 값 | 힙(할당자 기준)과 RSS, 단계별 | 활동 모니터의 "메모리"(physical footprint), 0.5초마다 |
| 조작 | 자동 | 사람이 직접(스크립트는 시작과 끝만 받음) |
| 운영체제 | macOS · Windows · Linux | macOS만 (`footprint`, `pgrep`) |

## 측정 순서

1. 재려는 앱을 띄우고 저장소를 연 뒤 히스토리가 보이게 둡니다. 시작점은 "저장소가 열리고 히스토리가 보이는 상태"입니다. 같은 앱의 다른 창이나 인스턴스는 닫습니다.
2. 스크립트를 실행합니다. 인자는 `/Applications`에 있는 앱 이름(`.app` 제외)입니다.

   ```bash
   tools/gui-bench/sample.sh "GitHub Desktop"
   tools/gui-bench/sample.sh gf
   INTERVAL=0.2 tools/gui-bench/sample.sh Fork   # 0.2초마다 (기본 0.5초)
   ```

3. Enter를 누르고 히스토리를 **마지막 커밋까지** 스크롤합니다.
4. 마지막 커밋이 화면에 보이는 순간 Enter를 다시 누릅니다.

## 출력

시간별 메모리(CSV `seconds,MB`) 뒤에 요약 한 줄이 나옵니다. 아래는 형식만 보이는 예이고, 측정값이 아닙니다.

```text
<앱 이름>
start <MB> MB, end <MB> MB, peak <MB> MB, <초> s
```

- 메모리는 `.app` 안의 **모든 프로세스**를 더한 값입니다. Electron 앱은 프로세스가 여러 개라 하나만 보면 적게 나옵니다.
- 시간은 두 Enter 사이입니다. 사람이 누르므로 수 초 단위로만 비교합니다.

## 비교할 때 지킬 것

- 같은 저장소, 같은 시작 상태(앱을 새로 띄운 직후)로 앱마다 2~3번 재고 중간값을 씁니다.
- README에 수치를 옮길 때는 저장소(커밋 수), 앱 버전, macOS 버전, 기기와 함께 적습니다(AGENTS.md "문서 갱신").
- gf는 `/Applications/gf.app`으로 설치된 릴리스 빌드를 잽니다. `cargo run`의 디버그 빌드는 프로세스 경로가 달라 잡히지 않고, 수치도 다릅니다.
