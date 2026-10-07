**Jev의 판단을 확인하고, 질문이 바뀔 때 답변이 어떻게 달라지는지 살펴보세요.**

# Jev Observer

[English](README.md) · [한국어](README.ko.md) · [웹사이트 및 제품 소개](https://jev-observer-web.vercel.app/)

Jev Observer는 **Jev와 System One 호환 모델**을 위한 로컬 프록시이자 대시보드입니다. 로컬 Laya 서버도 지원합니다. 요청과 답변을 저장해 모델의 판단을 살펴보고, 질문 정의를 비교하며, 실패·지연 시간·토큰 사용량·비용을 확인할 수 있습니다.

애플리케이션의 요청은 Observer를 거쳐 설정한 모델 제공업체로 전달됩니다. 저장된 기록은 브라우저에서 확인합니다.

```text
애플리케이션 → Jev Observer → 모델 제공업체 또는 로컬 Laya 서버
                    ↓
             로컬 기록 + 대시보드
```

Rust, SQLite, React로 개발되었으며, 대시보드와 글꼴을 포함한 **실행 파일 하나**로 동작합니다. Observer 계정이나 구독이 필요하지 않고, 분석 데이터 수집이나 이벤트 자동 업로드도 하지 않습니다. 실제 추론 요청은 설정한 제공업체로 전달되며, 해당 업체의 요금이 적용됩니다.

![요청 활동, 질문 그룹, 사용량, 수집 상태를 보여주는 Jev Observer 대시보드](docs/images/overview.png)

*오프라인 데모 화면입니다. 720개의 합성 요청을 사용하며 지연 시간, 사용량, 비용은 예시 데이터입니다.*

<details>
<summary>요청 상세 정보와 답변 확률 보기</summary>

![합성 라우팅 판단, 답변 확률, 검토 기능을 보여주는 요청 상세 화면](docs/images/request-details.png)

답변, 확률, 요청별 사용량을 확인하고 로컬 검토 라벨을 추가할 수 있습니다. 이 예시는 합성 데이터입니다.

</details>

## 주요 기능

- **판단 살펴보기:** Choice, Score, Noul 답변과 질문 정의, 모델이 보고한 확률을 확인합니다.
- **질문 버전 비교:** 반복되는 질문을 그룹으로 묶고, 정의가 바뀌면 통계를 별도로 유지합니다.
- **결과 검색 및 검토:** 질문 그룹을 검색하고 날짜·출처·모델로 기록을 필터링합니다. 답변에 정답·오답·알 수 없음 라벨을 붙일 수 있습니다.
- **사용량 및 비용 확인:** 요청별 토큰 사용량, OpenRouter가 보고한 USD 비용, 직접 설정한 추정 비용을 확인합니다. 한 요청에 여러 답변이 있어도 요청은 한 번만 집계합니다.
- **데이터 가져오기 및 내보내기:** Observer JSONL과 지원되는 JevRouter 영수증을 가져오고, 필터링한 기록을 JSONL 또는 CSV로 내보냅니다.
- **오프라인 체험:** 제공업체 인증 정보나 모델 호출 없이 데모를 사용합니다.

현재 Observer가 전달하는 경로는 `POST /v1/systemone`입니다. 모델을 직접 실행하거나 모든 제공업체를 라우팅하는 기능, 같은 입력을 여러 질문 버전에 재실행하는 기능은 제공하지 않습니다. [호환성 및 검증 범위](docs/compatibility.md)를 참고하세요.

## 빠른 시작: 데모 실행

[v0.2.1 다운로드](https://github.com/LimePencil/jev-observer/releases/tag/v0.2.1) 또는 아래 설치 스크립트를 이용하세요. 미리 빌드된 실행 파일은 Rust, Node.js, 데이터베이스 서버, 관리자 권한 없이 실행할 수 있습니다.

| 플랫폼 | 아키텍처 | 압축 형식 |
|---|---|---|
| Linux | x86-64 / ARM64 | `.tar.gz` (정적 musl 실행 파일) |
| macOS | Intel / Apple silicon | `.tar.gz` |
| Windows | x86-64 / ARM64 | `.zip` |

### Linux / macOS

```sh
curl -fsSL https://raw.githubusercontent.com/LimePencil/jev-observer/main/install.sh -o install.sh
sh install.sh --version 0.2.1
export PATH="$HOME/.local/bin:$PATH"
jev-observer --demo
```

### Windows PowerShell

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/LimePencil/jev-observer/main/install.ps1 -OutFile install.ps1
.\install.ps1 -Version 0.2.1
$env:PATH = "$env:LOCALAPPDATA\JevObserver\bin;$env:PATH"
jev-observer --demo
```

**[http://127.0.0.1:8765](http://127.0.0.1:8765)**에 접속해 로그인하세요.

| 항목 | 값 |
|---|---|
| 사용자 이름 | `observer` |
| 비밀번호 | `.jev-observer/observer.demo.access-token` 파일에 있는 토큰 |

Observer는 시작할 때 정확한 토큰 파일 경로를 출력합니다. 데모는 별도의 `*.demo.sqlite` 데이터베이스를 사용하며, 요청을 모델로 전달하지 않고 오프라인으로 동작합니다. **Ctrl-C**로 종료할 수 있습니다.

macOS와 Windows 실행 파일은 서명되지 않았습니다. 운영체제의 실행 제한, 비공개 저장소 접근, 업그레이드, 삭제 방법은 [설치 안내](docs/installation.md)를 참고하세요. 릴리스에는 `SHA256SUMS`가 포함됩니다. 변경 사항은 [0.2.1 릴리스 노트](docs/releases/0.2.1.md)에 정리되어 있습니다.

## 애플리케이션 연결

### 1. 데이터베이스 키 생성 및 보관

실제 요청 기록은 암호화됩니다. **32바이트 키**를 생성한 뒤 출력된 64자리 값을 비밀번호 관리자에 보관하고, 실제 요청을 수집할 때마다 `JEV_OBSERVER_DB_KEY`로 제공하세요. **키를 잃어버리면 암호화된 기록을 읽을 수 없습니다.** 이 키는 제공업체 API 키 및 대시보드 로그인 토큰과 별개입니다.

Bash (OpenSSL 필요):

```bash
# 한 번만 생성하고 출력된 값을 보관하세요.
openssl rand -hex 32

# 보관한 키를 화면에 표시하거나 셸 기록에 남기지 않고 입력합니다.
read -r -s -p 'Saved database key: ' JEV_OBSERVER_DB_KEY; echo
export JEV_OBSERVER_DB_KEY
```

<details>
<summary>Windows PowerShell 명령</summary>

```powershell
# 한 번만 생성하고 출력된 값을 보관하세요.
$keyBytes = New-Object byte[] 32
$random = [Security.Cryptography.RandomNumberGenerator]::Create()
$random.GetBytes($keyBytes)
$random.Dispose()
[BitConverter]::ToString($keyBytes).Replace('-', '').ToLowerInvariant()

# 보관한 키를 화면에 표시하지 않고 입력합니다.
$savedKey = Read-Host 'Saved database key' -AsSecureString
$env:JEV_OBSERVER_DB_KEY = [Net.NetworkCredential]::new('', $savedKey).Password
```

</details>

### 2. Observer 실행 및 제공업체 키 등록

애플리케이션 디렉터리에서 실행하거나, `--db /path/to/observer.sqlite`로 기록을 저장할 고정 경로를 지정하세요.

```sh
jev-observer
```

기본 요청 대상은 `https://api.typesafe.ai/v1/systemone`입니다. OpenRouter를 통해 Jev를 사용하려면 다음과 같이 실행하세요.

```sh
jev-observer --upstream https://openrouter.ai/api/v1/systemone
```

대시보드에서 사용자 이름 `observer`와 `.jev-observer/observer.access-token` 파일의 토큰으로 로그인하세요. 이 파일은 실제 요청 수집용 로그인 토큰을 담고 있습니다.

**Connect an application**에서 제공업체 키를 입력하고, 현재 세션에만 보관할지 운영체제의 자격 증명 저장소에 보관할지 선택하세요. 한 번 표시되는 **로컬 클라이언트 토큰**을 복사해 애플리케이션 환경의 `JEV_OBSERVER_CLIENT_TOKEN`으로 설정합니다. 세션에만 보관한 키는 Observer를 재시작하면 다시 등록해야 합니다.

### 3. SDK의 요청 대상을 Observer로 설정

기본 URL은 로컬 주소 **`http://127.0.0.1:8765`**로 지정하세요. 검증된 SDK는 `/v1/systemone` 경로를 자동으로 붙입니다. 기존 추론 호출은 유지하고, SDK 키에 로컬 클라이언트 토큰을 사용하세요.

**Python** — `typesafe-sdk==0.7.1`로 검증:

```python
import os
from typesafe_sdk import TypeSafeClient

client = TypeSafeClient(
    api_key=os.environ["JEV_OBSERVER_CLIENT_TOKEN"],
    base_url="http://127.0.0.1:8765",
    headers={
        "x-observer-source": "my-application",
        "Accept-Encoding": "identity",
    },
)
```

**JavaScript** — `@typesafe-ai/sdk@0.6.0`으로 검증:

```javascript
import { TypeSafeClient } from "@typesafe-ai/sdk";

const client = new TypeSafeClient({
  apiKey: process.env.JEV_OBSERVER_CLIENT_TOKEN,
  baseURL: "http://127.0.0.1:8765",
  defaultHeaders: {
    "x-observer-source": "my-application",
    "Accept-Encoding": "identity",
  },
});
```

Observer는 로컬 클라이언트 토큰을 확인한 뒤 등록된 제공업체 키로 바꾸어 요청을 전달합니다. `x-observer-source`는 기록에서 애플리케이션을 구분합니다. `Accept-Encoding: identity`는 구조화된 답변을 수집할 수 있도록 합니다. 압축 응답도 그대로 전달되지만, 저장된 수집 기록은 불완전한 것으로 표시됩니다.

제공업체 키를 직접 사용하는 인증 방식, 추가 메타데이터, 인증 정보 처리는 [연결 안내](docs/connection.md)를 참고하세요. [호환성 안내](docs/compatibility.md)에는 SDK 모의 서버 검증과 OpenRouter·Laya 실제 추론 검증이 구분되어 있습니다.

### 로컬 Laya 서버 사용

제공업체 키 없이 Laya가 루프백 주소에서 이미 실행 중이고, 보관한 데이터베이스 키를 설정한 상태라면 다음과 같이 실행하세요.

```sh
jev-observer --upstream http://127.0.0.1:8000/v1/systemone \
  --upstream-auth none --provider laya
```

`.jev-observer/observer.access-token`의 워크스페이스 토큰을 SDK 키로 사용하고, 요청 모델을 `english`로 지정하세요. Observer는 애플리케이션을 로컬에서 인증하며 Laya에는 인증 정보를 보내지 않습니다. CPU 추론은 SDK 타임아웃을 늘려야 할 수 있습니다. Laya 실행 방법, 키로 보호되는 서버, 호환성 제한은 [로컬 모델 설정](docs/connection.md#laya-and-other-local-system-one-models)을 참고하세요.

## 저장 및 개인정보 보호

| 설정 | 기본값 |
|---|---|
| 대시보드 및 프록시 | `127.0.0.1:8765` |
| 실제 요청 기록 | 실행한 디렉터리 기준 `.jev-observer/observer.sqlite` |
| 보관 기간 | 7일, 레코드 1,000,000개의 소프트 한도 적용 |
| 원본 입력 상태 저장 | 꺼짐. `--capture-state`로 활성화 |

실제 요청 기록은 SQLCipher로 암호화합니다. 알려진 인증 정보는 가려지지만, 저장된 질문 정의와 답변에는 민감한 내용이 남을 수 있습니다. **데모 데이터베이스와 JSONL/CSV 내보내기 파일은 평문입니다.** 기존 평문 데이터베이스를 마이그레이션하기 전에 이전 Observer 프로세스를 종료하세요.

알 수 없는 사용량과 비용은 그대로 알 수 없음으로 유지합니다. OpenRouter가 보고한 USD 비용은 추정값보다 우선합니다. 직접 비용을 추정하려면 USD 기준 `--input-price-per-million`과 `--output-price-per-million`을 모두 지정하세요. 추정값은 청구서나 자동 갱신되는 가격 정보가 아닙니다.

수집 상태에는 기록 실패와 수집·대기열 한도로 인한 기록 누락이 표시됩니다. 강제 종료하면 일부 기록이 유실될 수 있으며, Observer가 중지된 동안에는 요청이 통과할 수 없습니다. 백업, 민감 정보 가리기, 가져오기 한도, 종료 동작은 [저장·개인정보 보호·운영 한도](docs/storage.md)를 참고하세요.

## 빌드 및 개발

[rust-toolchain.toml](rust-toolchain.toml)에 고정된 Rust 도구 모음, 네이티브 C 컴파일러, npm이 포함된 **Node.js 22.12 이상**을 사용하세요. Windows는 번들 OpenSSL 빌드를 위해 Visual Studio C++ Build Tools, Windows 네이티브 Perl(예: Strawberry Perl), NASM도 필요합니다.

대시보드 파일을 실행 파일에 포함하려면 **Rust보다 먼저 대시보드를 빌드**해야 합니다.

```sh
npm ci --prefix ui
npm run build --prefix ui
cargo build --release --locked
./target/release/jev-observer --demo
```

Windows에서는 `.\target\release\jev-observer.exe --demo`를 실행하세요. `PATH`에 Git Bash Perl이 있다면 빌드 전에 `$env:OPENSSL_SRC_PERL`을 네이티브 Perl 실행 파일 경로로 설정하세요. Node.js는 개발과 빌드에만 필요합니다.

의존성을 설치하고 UI를 빌드한 뒤 검증 명령을 실행하세요.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-install.py
npx --prefix ui playwright install chromium
npm test --prefix ui
OBSERVER_BINARY="$PWD/target/release/jev-observer" npm run test:integration --prefix ui
```

마지막 명령은 Bash용이며, 제공업체 호출 없이 실제 로컬 백엔드와 내장 대시보드를 검증합니다. 프런트엔드 개발 시 백엔드를 8765 포트에서 실행하고 `npm run dev --prefix ui`를 사용하세요. [UI 개발 안내](ui/README.md)와 [CI 워크플로](.github/workflows/ci.yml)를 참고하세요.

## 문서

아래 상세 문서는 영어로 제공됩니다.

- [설치, 업그레이드 및 문제 해결](docs/installation.md)
- [애플리케이션 연결 및 인증 정보](docs/connection.md)
- [SDK 및 제공업체 호환성](docs/compatibility.md)
- [저장, 개인정보 보호 및 운영 한도](docs/storage.md)
- [성능 측정](docs/performance.md) 및 [스트레스 테스트](docs/stress.md)
- [구현 명세](docs/implementation-contract.md)

프로젝트 이력은 [연구 및 출시 계획](research/launch.md), [조사 자료](research/survey/README.md), [출처 목록](research/sources.md), [이전 보고서](reports/README.md), [검증 자료](reports/validation/README.md)를 참고하세요. 계획 문서에는 제안 단계의 작업도 포함되어 있으므로, 모든 항목이 구현된 기능은 아닙니다.

## 라이선스

Jev Observer는 독립 프로젝트이며 이름은 잠정적입니다. 프로젝트 자체 코드는 [MIT 라이선스](LICENSE)를 따르고, 번들 구성 요소와 글꼴은 각자의 라이선스를 유지합니다. 릴리스에는 `/licenses/jev-observer-MIT.txt`와 `/licenses/THIRD-PARTY-NOTICES.txt`에 고지문이 포함됩니다. [라이선스 관리](docs/licensing.md)를 참고하세요.
