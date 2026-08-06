#!/usr/bin/env python3
"""inno-creed 라이브 스모크 — 실제 아마란스(gw.innogrid.com)에 붙어 MCP 도구를 왕복시킨다.

⚠️ **이것은 진짜 회사 데이터를 건드린다** — 회의실이 실제로 예약되고, 개인 일정이
   생기고, 결재라인이 만들어지고, 본인 앞으로 메일이 발송된다. 전부 즉시 되돌리지만
   "잠깐 존재했다"는 사실 자체는 남는다. 그래서:

   · CI에서는 **절대** 돌지 않는다(아래 ① 게이트).
   · 사람이 매번 의도적으로 승인해야만 실행된다.

   실행법과 fixtures.json 작성법은 tests/live/README.md 참조.

안전장치 4겹
 ① **승인 게이트** — CI 환경변수 감지 시 즉시 중단 / `INNO_CREED_LIVE=<오늘 YYYYMMDD>` 필수
    (값이 오늘 날짜라 CI 설정에 상주시킬 수 없다) / 대화형 터미널이면 확인 문구까지 입력.
 ② **금지 도구 물리 차단** — `call()` 진입부에서 예외를 던진다. 주의 문구가 아니라 차단이다.
    더불어 서버의 `tools/list`와 대조해 **커버도 금지도 안 된 새 도구**가 있으면 FAIL.
 ③ **쓰기 대상 고정 + 마커 불변식** — 쓰기 좌표(회의실·과거 날짜)는 fixtures.json에서만 오고,
    생성물엔 전부 marker를 붙인다. 삭제/취소는 "우리가 만든 id" AND "marker가 붙어 있음"이
    둘 다 참일 때만 실행한다 → 남의 예약·일정을 지울 경로 자체가 없다.
 ④ **잔여물 대장** — 생성 즉시 leaks.json에 기록하고(kill -9로 죽어도 남는다), finally에서
    역순 정리하며 지운다. 정리 못 한 게 남으면 수동 정리 명령과 함께 남기고 exit 1.
    다음 실행은 시작 전에 그 대장부터 청소한다.

판정 규칙 (regress.py에서 이어받은 것)
 · rmcp는 인자 타입 오류를 JSON-RPC error가 아니라 `isError:true` **결과**로 준다 → 반드시 확인.
 · check 함수는 실제 필드를 본다. `lambda d: (True, "OK")` 같은 무조건 통과는 두지 않는다.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from datetime import date, datetime, timedelta

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
BIN = os.environ.get("INNO_CREED_BIN") or os.path.join(ROOT, "target", "release", "inno-creed")
FIXTURES = os.environ.get("INNO_CREED_LIVE_FIXTURES") or os.path.join(HERE, "fixtures.json")
LEDGER = os.path.join(HERE, "leaks.json")
OUTDIR = os.path.join(HERE, "out")

CONFIRM_PHRASE = "실행합니다"

# ── ① 승인 게이트 ────────────────────────────────────────────────────────────
# CI/자동화 러너가 심는 환경변수. 하나라도 있으면 무조건 거부한다.
CI_MARKERS = (
    "CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS", "GITLAB_CI", "JENKINS_URL",
    "JENKINS_HOME", "BUILD_NUMBER", "BUILD_ID", "TF_BUILD", "TEAMCITY_VERSION",
    "CIRCLECI", "BUILDKITE", "DRONE", "BITBUCKET_BUILD_NUMBER", "CODEBUILD_BUILD_ID",
    "APPVEYOR", "TRAVIS",
)

# ── ② 절대 호출하지 않는 도구 ────────────────────────────────────────────────
# 되돌릴 수 없거나, 되돌려도 다른 사람에게 흔적이 남는 것들.
FORBIDDEN = {
    "attendance_clock_in": "실제 근태 punch — 되돌릴 수 없다",
    "attendance_clock_out": "실제 근태 punch — 되돌릴 수 없다",
    "delete_temp_approval": "임시보관 문서 삭제 — 사용자의 진짜 초안을 지울 수 있다",
}

# 상신 시나리오는 별도 opt-in(`INNO_CREED_LIVE_SUBMIT=1`)일 때만 열린다.
# 되돌리기 자체는 완전하다 — 실측(2026-08-06, docId 141760): submit → appSq 33147 생성 →
# cancel(purge) 3단계 → **HP 근태 레코드까지 회수**(59건 → 59건), read_approval 2156.
# 그래도 기본에서 빼는 이유는 잔여물이 아니라 **사람**이다 — 결재선의 3명에게 매 실행 알림이 간다.
SUBMIT_TOOLS = {
    "submit_approval": "실제 결재 상신 — 결재선의 사람들에게 알림이 간다(INNO_CREED_LIVE_SUBMIT=1 필요)",
    "cancel_approval": "상신 문서 회수 — 상신 시나리오와 짝이다(INNO_CREED_LIVE_SUBMIT=1 필요)",
}


def submit_enabled() -> bool:
    return os.environ.get("INNO_CREED_LIVE_SUBMIT") == "1"


def die(msg: str, code: int = 2):
    print(f"\n⛔ {msg}\n", file=sys.stderr)
    sys.exit(code)


def assert_consent():
    """CI 차단 → 날짜 env → (TTY면) 확인 문구. 셋 다 통과해야 진행한다."""
    hits = [k for k in CI_MARKERS if os.environ.get(k)]
    if hits:
        die(
            "CI 환경으로 판단돼 실행을 거부한다.\n"
            f"   감지된 환경변수: {', '.join(hits)}\n"
            "   이 스크립트는 실제 회사 데이터를 변경한다 — 자동 파이프라인에서 돌려선 안 된다."
        )

    today = date.today().strftime("%Y%m%d")
    if os.environ.get("INNO_CREED_LIVE") != today:
        die(
            "명시적 승인이 없다.\n"
            f"   실행하려면:  INNO_CREED_LIVE={today} python3 tests/live/live_test.py\n"
            "   (값은 '오늘 날짜'다. 하루 지나면 무효라 CI 설정에 박아둘 수 없다.)"
        )

    if sys.stdin.isatty():
        print("이 테스트는 실제 아마란스에 회의실 예약·일정·결재라인·메일을 만들었다 지운다.")
        print(f"진행하려면 다음을 그대로 입력: {CONFIRM_PHRASE}")
        try:
            got = input("> ").strip()
        except (EOFError, KeyboardInterrupt):
            die("입력 취소")
        if got != CONFIRM_PHRASE:
            die(f"확인 문구가 다르다({got!r}) — 실행하지 않는다.")


# ── fixtures ────────────────────────────────────────────────────────────────
def load_fixtures() -> dict:
    if not os.path.exists(FIXTURES):
        die(
            f"fixtures 파일이 없다: {FIXTURES}\n"
            "   tests/live/fixtures.example.json 을 복사해 본인 환경 값으로 채울 것.\n"
            "   (개인값이 들어가므로 이 파일은 git에 올라가지 않는다)",
            code=2,
        )
    fx = json.load(open(FIXTURES, encoding="utf-8"))

    def need(path):
        cur = fx
        for k in path.split("."):
            if not isinstance(cur, dict) or k not in cur:
                die(f"fixtures.json 에 '{path}' 가 없다 — fixtures.example.json 참조")
            cur = cur[k]
        return cur

    for p in ("marker", "self.empSeq", "self.name", "room.resSeq", "room.displayTitle",
              "approvalLine.formId", "approvalLine.docType", "search.query", "daysBack"):
        need(p)

    if not str(fx["marker"]).strip():
        die("fixtures.marker 가 비어 있다 — 마커가 없으면 삭제 가드가 동작하지 않는다")
    if not 14 <= int(fx["daysBack"]) <= 90:
        die(f"fixtures.daysBack={fx['daysBack']} — 14~90 사이여야 한다(너무 최근이면 눈에 띄고, 너무 오래면 조회 범위를 벗어난다)")
    return fx


# ── MCP 클라이언트 ───────────────────────────────────────────────────────────
class Mcp:
    def __init__(self, binary: str):
        if not os.path.exists(binary):
            die(f"바이너리가 없다: {binary}\n   먼저 `cargo build --release` 를 돌릴 것.")
        self.p = subprocess.Popen(
            [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1,
        )
        self.id = 0
        self.called: set[str] = set()
        self._rpc("initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "inno-creed-live", "version": "1"},
        })
        self._write({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _write(self, obj):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()

    def _rpc(self, method, params):
        self.id += 1
        want = self.id
        self._write({"jsonrpc": "2.0", "id": want, "method": method, "params": params})
        for line in self.p.stdout:
            try:
                m = json.loads(line)
            except Exception:
                continue
            if m.get("id") == want:
                return m
        raise RuntimeError(f"{method}: 응답 없음 (서버가 죽었을 수 있다)")

    def tool_names(self) -> list[str]:
        m = self._rpc("tools/list", {})
        return sorted(t["name"] for t in m["result"]["tools"])

    def call(self, name, **args):
        """(상태, 값[, 길이]) — 상태는 'OK' | 'ERR'."""
        blocked = dict(FORBIDDEN)
        if not submit_enabled():
            blocked.update(SUBMIT_TOOLS)
        if name in blocked:
            # ② 물리 차단. 이 예외는 잡지 않는다 — 테스트를 즉시 죽이는 것이 의도다.
            raise AssertionError(f"금지 도구 호출 시도: {name} ({blocked[name]})")
        if name == "send_mail" and args.get("to"):
            raise AssertionError("send_mail 은 본인 앞으로만 보낸다 — to 를 지정할 수 없다")
        self.called.add(name)
        m = self._rpc("tools/call", {"name": name, "arguments": args})
        if m.get("error"):
            e = m["error"]
            return ("ERR", f"rpc {e.get('code')} {str(e.get('message', ''))[:120]}")
        r = m["result"]
        txt = r["content"][0]["text"] if r.get("content") else ""
        if r.get("isError"):  # ⚠️ rmcp는 인자 오류를 여기로 준다
            return ("ERR", f"isError: {txt[:120]}")
        try:
            return ("OK", json.loads(txt), len(txt))
        except Exception:
            return ("OK", txt, len(txt))

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


# ── ④ 잔여물 대장 ────────────────────────────────────────────────────────────
PENDING: list[dict] = []


def save_ledger():
    """생성/정리 때마다 즉시 디스크에 반영 — 프로세스가 강제 종료돼도 흔적이 남게."""
    if PENDING:
        os.makedirs(HERE, exist_ok=True)
        json.dump(PENDING, open(LEDGER, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    elif os.path.exists(LEDGER):
        os.remove(LEDGER)


def track(kind: str, ref: dict, hint: str) -> dict:
    entry = {"kind": kind, "ref": ref, "hint": hint,
             "createdAt": datetime.now().strftime("%Y-%m-%d %H:%M:%S")}
    PENDING.append(entry)
    save_ledger()
    return entry


def untrack(entry: dict | None):
    if entry and entry in PENDING:
        PENDING.remove(entry)
        save_ledger()


# ③ 마커 불변식 — 되돌리기는 전부 이 함수들을 거친다. 우리가 만든 id 이면서
#    marker 가 붙어 있을 때만 실제 삭제 API를 부른다. 하나라도 어긋나면 손대지 않는다.
def undo_reservation(mcp: Mcp, ref: dict, marker: str):
    got = mcp.call("my_reservations", start=ref["date"], end=ref["date"])
    if got[0] != "ERR":
        row = next((r for r in got[1]["reservations"]
                    if str(r.get("seqNum")) == str(ref["seqNum"])), None)
        if row is None:
            return True, "이미 없음"
        if marker not in str(row.get("title", "")):
            return False, f"마커 없는 예약이라 건드리지 않음: {row.get('title')!r}"
        ref = {**ref, "resIdx": row.get("resIdx") or ref.get("resIdx") or "1"}
    r = mcp.call("cancel_reservation", res_seq=ref["resSeq"],
                 seq_num=ref["seqNum"], res_idx=str(ref.get("resIdx", "1")))
    if r[0] == "ERR":
        return False, r[1]
    return bool(r[1].get("ok")), "취소됨"


def undo_event(mcp: Mcp, ref: dict, marker: str):
    got = mcp.call("list_events", start=ref["date"], end=ref["date"])
    if got[0] != "ERR":
        row = next((e for e in got[1]["events"]
                    if str(e.get("schSeq")) == str(ref["schSeq"])), None)
        if row is None:
            return True, "이미 없음"
        if marker not in str(row.get("title", "")):
            return False, f"마커 없는 일정이라 건드리지 않음: {row.get('title')!r}"
    r = mcp.call("delete_calendar_event", sch_seq=str(ref["schSeq"]), date=ref["date"])
    if r[0] == "ERR":
        return False, r[1]
    return bool(r[1].get("ok")), "삭제됨"


def undo_approval_line(mcp: Mcp, ref: dict, marker: str):
    got = mcp.call("list_approval_lines")
    if got[0] == "ERR":
        return False, got[1]
    row = next((x for x in got[1]["lines"] if str(x["lineId"]) == str(ref["lineId"])), None)
    if row is None:
        return True, "이미 없음"
    if marker not in str(row.get("lineName", "")):
        return False, f"마커 없는 결재라인이라 건드리지 않음: {row.get('lineName')!r}"
    r = mcp.call("delete_approval_line", row_json=json.dumps(row["_row"], ensure_ascii=False))
    if r[0] == "ERR":
        return False, r[1]
    after = mcp.call("list_approval_lines")
    gone = after[0] != "ERR" and not any(
        str(x["lineId"]) == str(ref["lineId"]) for x in after[1]["lines"])
    return gone, "삭제됨(재조회 확인)" if gone else "삭제 호출은 됐으나 아직 목록에 남음"


MAIL_POLL_TRIES = 6      # 발송 직후 배달 대기 — 5초 × 6 = 최대 30초
MAIL_POLL_INTERVAL = 5
MAIL_SETTLED_SEC = 600   # 이 시간이 지난 항목은 "안 보이면 이미 정리된 것"으로 본다


def _find_mail(mcp: Mcp, subject: str):
    got = mcp.call("list_mail_inbox")
    if got[0] == "ERR":
        return "ERR", got[1]
    return "OK", next((m for m in got[1]["Records"]
                       if subject in str(m.get("subject", ""))), None)


def undo_mail(mcp: Mcp, ref: dict, marker: str):
    """⚠️ 발송 직후에는 서버가 아직 배달하지 않아 받은메일함에서 안 보일 수 있다.
    '안 보임 = 정리 완료'로 낙관하면 잠시 후 도착한 메일이 그대로 남는다 → 기다렸다 다시 본다."""
    try:
        age = (datetime.now() - datetime.strptime(ref["sentAt"], "%Y-%m-%d %H:%M:%S")).total_seconds()
    except Exception:
        age = MAIL_SETTLED_SEC + 1  # sentAt 이 없거나 깨졌으면 오래된 항목으로 취급
    settled = age > MAIL_SETTLED_SEC
    tries = 1 if settled else MAIL_POLL_TRIES

    hit = None
    for i in range(tries):
        st, hit = _find_mail(mcp, ref["subject"])
        if st == "ERR":
            return False, hit
        if hit:
            break
        if i < tries - 1:
            time.sleep(MAIL_POLL_INTERVAL)

    if hit is None:
        if settled:
            return True, "받은메일함에 없음(오래된 항목 — 이미 정리된 것으로 본다)"
        return False, (f"발송 후 {MAIL_POLL_TRIES * MAIL_POLL_INTERVAL}초를 기다려도 받은메일함에 "
                       "나타나지 않음 — 늦게 도착할 수 있으니 수동 확인 필요")

    if marker not in str(hit.get("subject", "")):
        return False, "마커 없는 메일이라 건드리지 않음"
    r = mcp.call("delete_mail", uids=str(hit["muid"]))
    if r[0] == "ERR":
        return False, r[1]
    st, still = _find_mail(mcp, ref["subject"])  # read-back — successTf 를 믿지 않는다
    if st == "ERR":
        return False, still
    if still is not None:
        return False, f"delete_mail 은 성공했다는데 muid={hit['muid']} 가 받은메일함에 아직 있음"
    return True, f"muid={hit['muid']} 휴지통 이동(재조회 확인)"


def undo_approval(mcp: Mcp, ref: dict, marker: str):
    """상신 문서 회수 — 결재취소(30→20) → 상신취소(20→10) → 임시보관삭제(10→소멸).
    ⚠️ 이 3단계는 HP 근태 레코드(appSq)까지 회수한다(2026-08-06 실측, docId 141760)."""
    doc_id, form_id = str(ref["docId"]), str(ref["formId"])
    got = mcp.call("list_approvals", box_name="sent", page_size=50)
    if got[0] != "ERR":
        row = next((d for d in got[1].get("documents", [])
                    if str(d.get("docId")) == doc_id), None)
        if row is not None and marker not in str(row.get("title", "")):
            return False, f"마커 없는 문서라 건드리지 않음: {row.get('title')!r}"
    r = mcp.call("cancel_approval", doc_id=doc_id, form_id=form_id, purge=True)
    if r[0] == "ERR":
        return False, r[1]
    # read-back — 삭제된 문서는 eap111A04가 2156으로 거절한다(그게 정상 종착지다).
    chk = mcp.call("read_approval", doc_id=doc_id, form_id=form_id)
    if chk[0] == "ERR" and "2156" in str(chk[1]):
        return True, f"삭제 확인(2156) · steps={r[1].get('steps')}"
    after = mcp.call("list_approvals", box_name="sent", page_size=50)
    if after[0] != "ERR" and not any(str(d.get("docId")) == doc_id
                                     for d in after[1].get("documents", [])):
        return True, f"상신함에서 사라짐 · steps={r[1].get('steps')}"
    return False, "취소는 실행됐으나 삭제가 확인되지 않음"


UNDO = {
    "reservation": undo_reservation,
    "event": undo_event,
    "approval_line": undo_approval_line,
    "mail": undo_mail,
    "approval": undo_approval,
}


def cleanup(mcp: Mcp, marker: str, label: str) -> list[dict]:
    """PENDING 을 역순으로 정리한다. 실패분만 남겨 돌려준다."""
    if not PENDING:
        return []
    print(f"\n── {label} ({len(PENDING)}건) ──")
    for entry in list(reversed(PENDING)):
        fn = UNDO.get(entry["kind"])
        try:
            ok, note = fn(mcp, entry["ref"], marker) if fn else (False, "알 수 없는 kind")
        except Exception as e:
            ok, note = False, f"{type(e).__name__}: {e}"
        print(f" {'✅' if ok else '❌'} {entry['kind']:14} {note}")
        if ok:
            untrack(entry)
    save_ledger()
    return list(PENDING)


# ── 결과 집계 ────────────────────────────────────────────────────────────────
R: list[tuple[str, str, str]] = []


def run(mcp: Mcp, name, check, **args):
    st = mcp.call(name, **args)
    if st[0] == "ERR":
        R.append(("FAIL", name, st[1]))
        return None
    data, size = st[1], st[2]
    try:
        ok, extra = check(data)
    except Exception as e:
        R.append(("FAIL", name, f"검증 예외 {type(e).__name__}: {e}"))
        return data
    R.append(("PASS" if ok else "FAIL", name, f"{size}자 · {extra}"))
    return data


def skip(name, why):
    R.append(("SKIP", name, why))


# ── 쓰기 좌표 선정 ───────────────────────────────────────────────────────────
def past_weekday(days_back: int) -> date:
    d = date.today() - timedelta(days=days_back)
    while d.weekday() >= 5:  # 토/일이면 앞의 평일로
        d -= timedelta(days=1)
    return d


PROBE = os.path.join(ROOT, "target", "release", "probe")


def hp_records() -> dict | None:
    """HP 근태신청 레코드(appSq → atDt). MCP 도구로 노출돼 있지 않아 probe 바이너리로 읽는다.
    상신이 **실패**하면 create가 만든 HP 레코드만 남고 취소할 eap 문서가 없어 **지울 방법이 없다**
    (하드삭제 API 부재 — 07 §10.5). 그래서 상신 전후로 이걸 찍어 고아 발생을 감지한다."""
    if not os.path.exists(PROBE):
        return None
    body = json.dumps({"approStateList": ["0", "1", "2", "3", "4", "5"], "linkAtCdList": [],
                       "startDate": "20260101", "endDate": "20301231",
                       "calendarViewType": "DEFAULT"})
    try:
        out = subprocess.run([PROBE, "/human/attendapplication/at00001", body],
                             capture_output=True, text=True, timeout=60)
        rows = json.loads(out.stdout)["response"].get("resultData") or []
    except Exception:
        return None
    return {int(r["appSq"]): str(r.get("atDt", "")) for r in rows if r.get("appSq") is not None}


def _probe_a03(form_id, line_id) -> dict | None:
    """상신하지 않고 **병합된 결재선**을 미리 본다(eap110A03는 읽기 콜, docID=0)."""
    if not os.path.exists(PROBE):
        return None
    body = json.dumps({"docID": 0, "formID": str(form_id),
                       "approkey": "ERP_00000000-0000-0000-0000-000000000000",
                       "appLineId": str(line_id), "draftTp": "", "reDraft": "", "docType": "",
                       "doc_auth": 0, "pageCode": "UBAP001"})
    try:
        out = subprocess.run([PROBE, "/eap/eap110A03", body],
                             capture_output=True, text=True, timeout=60)
        rm = (json.loads(out.stdout)["response"].get("resultData") or {}).get("resultMap") or {}
    except Exception:
        return None

    def names(key):
        v = rm.get(key) or []
        if isinstance(v, dict):
            v = [v]
        return [str(x.get("user_nm") or x.get("emp_nm") or x.get("user_id")) for x in v]

    return {"approvers": names("kyuljaeResult"), "refer": names("m_Refer"),
            "oper": names("m_Oper"),
            "formDTp": ((rm.get("form_info") or {}).get("form_d_tp") or "")}


def _minutes(iso_ts: str) -> int | None:
    # "2026-07-06T10:00" → 600
    try:
        hh, mm = iso_ts.split("T")[1].split(":")[:2]
        return int(hh) * 60 + int(mm)
    except Exception:
        return None


def pick_slot(mcp: Mcp, res_seq: str, days_back: int, windows) -> str | None:
    """과거 평일 중 해당 회의실의 대상 구간이 **비어 있는** 날을 고른다.
    남의 실제 예약과 겹칠 일을 만들지 않기 위한 사전 확인이다."""
    for back in range(days_back, days_back + 14):
        d = past_weekday(back)
        gap = (date.today() - d).days
        if not 14 <= gap <= 90:
            continue
        ymd = d.strftime("%Y%m%d")
        got = mcp.call("list_reservations", start=ymd, end=ymd, res_seqs=[res_seq])
        if got[0] == "ERR":
            return None
        busy = []
        for r in got[1]["reservations"]:
            a, b = _minutes(str(r.get("start", ""))), _minutes(str(r.get("end", "")))
            if a is not None and b is not None:
                busy.append((a, b))
        if all(not (a < w[1] and b > w[0]) for a, b in busy for w in windows):
            return ymd
    return None


# ── 본체 ─────────────────────────────────────────────────────────────────────
def main():
    assert_consent()
    fx = load_fixtures()
    marker = fx["marker"]
    os.makedirs(OUTDIR, exist_ok=True)

    mcp = Mcp(BIN)
    leaked: list[dict] = []
    try:
        # 지난 실행이 남긴 잔여물부터 청소한다.
        if os.path.exists(LEDGER):
            PENDING.extend(json.load(open(LEDGER, encoding="utf-8")))
            print(f"⚠️ 지난 실행의 잔여물 {len(PENDING)}건 발견 — 먼저 정리한다.")
            cleanup(mcp, marker, "이전 잔여물 정리")

        surface = mcp.tool_names()
        body(mcp, fx, marker)
    finally:
        leaked = cleanup(mcp, marker, "생성물 정리")
        # ② 커버리지 역검사 — 호출도 안 하고 금지도 안 한 도구가 있으면 잡는다.
        try:
            covered = mcp.called | set(FORBIDDEN) | {n for st, n, _ in R if st == "SKIP"}
            uncovered = [n for n in surface if n not in covered]
            missing_forbidden = [n for n in FORBIDDEN if n not in surface]
            if uncovered:
                R.append(("FAIL", "도구_커버리지", f"점검 안 된 도구 {len(uncovered)}개: {', '.join(uncovered)}"))
            else:
                R.append(("PASS", "도구_커버리지", f"{len(surface)}개 전부 커버(금지 {len(FORBIDDEN)}개 포함)"))
            if missing_forbidden:
                R.append(("FAIL", "금지목록_정합성",
                          f"금지 목록에 있으나 서버에 없는 도구: {', '.join(missing_forbidden)}"))
        except NameError:
            pass  # tools/list 도 못 한 채 죽은 경우
        mcp.close()

    json.dump(R, open(os.path.join(OUTDIR, "result.json"), "w", encoding="utf-8"),
              ensure_ascii=False, indent=1)
    p = sum(1 for x in R if x[0] == "PASS")
    f = sum(1 for x in R if x[0] == "FAIL")
    s = sum(1 for x in R if x[0] == "SKIP")
    print(f"\nPASS {p} · FAIL {f} · SKIP {s}")
    for st, n, note in R:
        print(f" {'✅' if st == 'PASS' else '❌' if st == 'FAIL' else '⏭ '} {n:32} {note}")

    if leaked:
        print(f"\n⚠️ 정리 못 한 잔여물 {len(leaked)}건 — {LEDGER} 에 기록했다. 수동 정리 필요:")
        for e in leaked:
            print(f"   · {e['kind']} {e['ref']} → {e['hint']}")
    sys.exit(1 if (f or leaked) else 0)


def body(mcp: Mcp, fx: dict, marker: str):
    today = date.today().strftime("%Y%m%d")
    room = fx["room"]["resSeq"]

    # ── 1. 조회 ──────────────────────────────────────────────────────────────
    me = run(mcp, "whoami", lambda d: (
        bool(d["empSeq"]) and bool(d["compSeq"]) and bool(d["empCd"])
        and str(d["empSeq"]) == str(fx["self"]["empSeq"]),
        f"empSeq={d['empSeq']} {d['deptName']}/{d['duty']}"))
    if not me:
        raise RuntimeError("whoami 실패 — 크레덴셜(브라우저 로그인)을 확인할 것")

    run(mcp, "list_resources", lambda d: (
        any(str(r.get("resSeq")) == room for r in d["resultList"]),
        f"자원 {len(d['resultList'])}개 · fixture 회의실({room}) 존재"))
    run(mcp, "list_reservations", lambda d: (
        isinstance(d["reservations"], list) and d["count"] == len(d["reservations"])
        and all("title" in r and "displayTitle" in r for r in d["reservations"]),
        f"{d['count']}건 · title/displayTitle 존재"), start=today, end=today)

    def chk_free(d):
        bad = [s for r in d["rooms"] for s in r["freeSlots"]
               if s["from"] < "14:00" and s["to"] > "13:00"]
        return (d["lunchExcluded"] is True and not bad and d["lunchBreak"] == "13:00~14:00",
                f"{d['roomsWithSlot']}/{d['roomsChecked']}실 · 점심걸친슬롯 {len(bad)}개")

    run(mcp, "find_free_rooms", chk_free, date=today, duration_min=60, window="1100-1700")
    run(mcp, "find_free_rooms", lambda d: (
        d["lunchExcluded"] is False
        and any(s["from"] < "14:00" and s["to"] > "13:00"
                for r in d["rooms"] for s in r["freeSlots"]),
        "점심 포함 슬롯 존재"),
        date=today, duration_min=60, window="1100-1700", include_lunch=True)
    run(mcp, "my_reservations", lambda d: (
        d["kind"] == "myReservations" and isinstance(d["reservations"], list),
        f"{d['count']}건"), start=today, end=today)

    run(mcp, "list_calendars", lambda d: (len(d["resultList"]) > 0, f"캘린더 {len(d['resultList'])}개"))
    run(mcp, "list_events", lambda d: (
        d["count"] == len(d["events"])
        and all({"schSeq", "title", "start", "mine"} <= set(e) for e in d["events"]),
        f"{d['count']}건 · mine 필드 존재"), start=today, end=today)

    run(mcp, "list_mailboxes", lambda d: (
        isinstance(d, (list, dict)) and len(d) > 0, f"메일함 {len(d)}개 항목"))
    inbox = run(mcp, "list_mail_inbox", lambda d: (isinstance(d["Records"], list), f"{len(d['Records'])}통"))
    notices = run(mcp, "list_notices", lambda d: (
        len(d["articles"]) > 0, f"{len(d['articles'])}건/전체 {d['totalCnt']}"), page_size=5)

    run(mcp, "pending_approvals", lambda d: (
        "count" in d or "approvals" in d or "documents" in d, "함 구조 반환"), page_size=5)
    run(mcp, "list_approvals", lambda d: (
        isinstance(d["documents"], list) and d["box"] == "pending",
        f"미결 {d['totalCount']}건"), box_name="pending", page_size=5)
    ref = run(mcp, "list_approvals", lambda d: (
        isinstance(d["documents"], list) and d["box"] == "reference",
        f"수신참조 {d['totalCount']}건"), box_name="reference", page_size=5)
    run(mcp, "approval_counts", lambda d: (len(d) >= 3, f"{len(d)}개 함"))
    lines = run(mcp, "list_approval_lines", lambda d: (
        isinstance(d["lines"], list) and all("_row" in x for x in d["lines"]),
        f"{len(d['lines'])}개 · _row 보유"))
    run(mcp, "list_approval_line_schemas", lambda d: (len(json.dumps(d)) > 100, "목록 반환"))
    doc_type = fx["approvalLine"]["docType"]
    run(mcp, "get_approval_line_schema", lambda d: (
        doc_type[:2] in json.dumps(d, ensure_ascii=False), f"{doc_type} 스키마"), doc_type=doc_type)
    run(mcp, "list_approval_submission_guides", lambda d: (len(json.dumps(d)) > 100, "목록 반환"))
    run(mcp, "get_approval_submission_guide", lambda d: (
        "draftHelp" in json.dumps(d), "draftHelp 포함"), doc_type=doc_type)

    def chk_suggest(d):
        steps = [s for b in d["branches"] for s in b["steps"]]
        return (d["verificationRequired"] is True and bool(steps)
                and all("candidates" in s for s in steps),
                f"{len(steps)}단계 전부 candidates 보유 · 검증필요 표시")

    run(mcp, "suggest_approval_line", chk_suggest, doc_type=doc_type)

    run(mcp, "get_attendance_today", lambda d: (len(json.dumps(d)) > 20, "반환"))
    run(mcp, "attendance_month", lambda d: (len(json.dumps(d)) > 100, "반환"),
        month=today[:6])
    run(mcp, "org_chart", lambda d: (len(json.dumps(d)) > 500, "반환"))
    run(mcp, "find_person", lambda d: (
        str(fx["self"]["empSeq"]) in json.dumps(d), "empSeq 반환"), query=fx["self"]["name"])
    run(mcp, "search", lambda d: (len(json.dumps(d)) > 200, "반환"),
        query=fx["search"]["query"], limit=5)

    # ── 2. ID 물린 상세 ──────────────────────────────────────────────────────
    if inbox and inbox["Records"]:
        muid = inbox["Records"][0]["muid"]
        run(mcp, "read_mail", lambda d: (len(json.dumps(d)) > 100, "본문 반환"), muid=str(muid))
        att = [m for m in inbox["Records"] if m.get("attach")]
        sn = None  # ⚠️ file_sn 은 순번이 아니라 read_mail 이 주는 **서버 토큰**
        if att:
            rm = mcp.call("read_mail", muid=str(att[0]["muid"]))
            if rm[0] == "OK" and rm[1].get("attachments"):
                sn = rm[1]["attachments"][0]["fileSn"]
        if sn:
            run(mcp, "download_mail_attachment", lambda d: (
                d["ok"] and d["bytes"] > 0, f"{d['serverFileName']} {d['bytes']}B"),
                muid=str(att[0]["muid"]), file_sn=sn,
                out_path=os.path.join(OUTDIR, "mail_att.bin"))
        else:
            skip("download_mail_attachment", "받은메일함에 첨부 있는 메일 없음")
    else:
        skip("read_mail", "받은메일함 비어 있음")
        skip("download_mail_attachment", "받은메일함 비어 있음")

    if notices and notices["articles"]:
        a0 = notices["articles"][0]
        run(mcp, "read_notice", lambda d: (len(json.dumps(d)) > 100, "본문 반환 ⚠️조회수+1"),
            art_seq_no=str(a0["artSeqNo"]))
        # ⚠️ fileCnt 는 **문자열**("0"도 truthy). 첨부 있는 글은 페이지를 넓혀 찾는다.
        big = mcp.call("list_notices", page_size=30)
        pool = big[1]["articles"] if big[0] == "OK" else notices["articles"]
        withfile = [a for a in pool if str(a.get("fileCnt", "0")) not in ("0", "")]
        if withfile:
            tgt = withfile[0]
            al = run(mcp, "list_notice_attachments", lambda d: (
                len(d["files"]) > 0, f"{tgt['title'][:18]} · {len(d['files'])}개"),
                art_seq_no=str(tgt["artSeqNo"]), uid=str(tgt["attachmentUid"]))
            if al and al.get("files"):
                run(mcp, "download_notice_attachment", lambda d: (
                    d["ok"] and d["bytes"] > 0, f"{d['serverFileName']} {d['bytes']}B"),
                    art_seq_no=str(tgt["artSeqNo"]), uid=str(tgt["attachmentUid"]),
                    file_sn=0, out_path=os.path.join(OUTDIR, "board_att.bin"))
            else:
                skip("download_notice_attachment", "첨부 목록이 비어 대상 없음")
        else:
            skip("list_notice_attachments", "첨부 있는 게시글 없음")
            skip("download_notice_attachment", "첨부 있는 게시글 없음")
    else:
        for n in ("read_notice", "list_notice_attachments", "download_notice_attachment"):
            skip(n, "공지 목록 비어 있음")

    docs = (ref or {}).get("documents") or []
    if docs and docs[0].get("docId"):
        d0 = docs[0]
        run(mcp, "read_approval", lambda d: (len(json.dumps(d)) > 200, "본문 반환"),
            doc_id=str(d0["docId"]), form_id=str(d0.get("formId", "")))
    else:
        skip("read_approval", "수신참조함에 문서 없음")

    if lines and lines["lines"]:
        run(mcp, "read_approval_line", lambda d: (len(json.dumps(d)) > 100, "members 반환"),
            line_id=str(lines["lines"][0]["lineId"]))
    else:
        skip("read_approval_line", "개인결재라인 없음")

    # ── 3. 되돌릴 수 있는 쓰기 ───────────────────────────────────────────────
    # ③ 좌표는 fixture + 사전 확인으로 정한다. 남의 예약과 겹치는 날은 애초에 고르지 않는다.
    slot_windows = [(600, 660), (750, 810)]  # 10:00~11:00(등록), 12:30~13:30(수정)
    past = pick_slot(mcp, room, int(fx["daysBack"]), slot_windows)
    if not past:
        for n in ("reserve_resource", "update_reservation", "cancel_reservation"):
            skip(n, "대상 구간이 빈 과거 평일을 못 찾음 — 남의 예약과 겹칠 수 있어 쓰기 생략")
        for n in ("create_calendar_event", "update_calendar_event", "delete_calendar_event"):
            skip(n, "과거 날짜 선정 실패")
    else:
        title = f"{marker} 라이브 점검"
        entry = None
        r = run(mcp, "reserve_resource", lambda d: (
            d["verified_by_readback"] and d["reqText"] == title
            and d["displayTitle"] == fx["room"]["displayTitle"] and "lunchWarning" not in d,
            f"{past} seq={d['seqNum']} displayTitle={d['displayTitle']} 경고없음"),
            res_seq=room, req_text=title, start=past + "1000", end=past + "1100",
            desc="자동 점검 — 즉시 취소됨")
        if r:
            entry = track("reservation",
                          {"resSeq": room, "seqNum": r["seqNum"], "resIdx": r.get("resIdx", "1"),
                           "date": past},
                          f"아마란스 회의실 예약 화면에서 {past} '{title}' 취소")
            u = run(mcp, "update_reservation", lambda d: (
                "lunchWarning" in d and d["reissued"] and d["verified_by_readback"],
                f"{d['prev_seqNum']}→{d['seqNum']} 재발급 + lunchWarning"),
                res_seq=room, seq_num=r["seqNum"], res_idx=r["resIdx"],
                start=past + "1230", end=past + "1330")
            if u:  # 수정은 seqNum 을 재발급한다 — 대장의 좌표를 갱신해야 정리가 된다
                untrack(entry)
                entry = track("reservation",
                              {"resSeq": room, "seqNum": u["seqNum"],
                               "resIdx": u.get("resIdx", "1"), "date": past},
                              f"아마란스 회의실 예약 화면에서 {past} '{title}' 취소")
            ok, note = undo_reservation(mcp, entry["ref"], marker)
            R.append(("PASS" if ok else "FAIL", "cancel_reservation", note))
            if ok:
                untrack(entry)
        else:
            skip("update_reservation", "등록 실패")
            skip("cancel_reservation", "등록 실패")

        e = run(mcp, "create_calendar_event", lambda d: (
            d["verified_by_readback"] and "개인캘린더" in d["calendar"],
            f"schSeq={d['schSeq']} {d['calendar']}"),
            title=title, start=past + "1000", end=past + "1100", contents="자동 점검 — 즉시 삭제됨")
        if e:
            ev = track("event", {"schSeq": e["schSeq"], "date": past},
                       f"아마란스 캘린더 {past} '{title}' 삭제")
            run(mcp, "update_calendar_event", lambda d: (
                str(d["schSeq"]) == str(e["schSeq"]) and d["title"] == title + "(수정)",
                "schSeq 유지(in-place)"),
                sch_seq=e["schSeq"], date=past, title=title + "(수정)")
            ok, note = undo_event(mcp, ev["ref"], marker)
            R.append(("PASS" if ok else "FAIL", "delete_calendar_event", note))
            if ok:
                untrack(ev)
        else:
            skip("update_calendar_event", "등록 실패")
            skip("delete_calendar_event", "등록 실패")

    # 결재라인 — 생성 후 즉시 삭제. 상신하지 않으므로 아무에게도 통지되지 않는다.
    line_nm = f"{marker} 라이브 점검"
    sl = run(mcp, "save_approval_line", lambda d: (
        d["createdLineId"] > 0, f"lineId={d['createdLineId']}"),
        form_id=fx["approvalLine"]["formId"], line_nm=line_nm,
        detail_line_json=json.dumps([{"user_id": me["empSeq"]}]))
    if sl and sl.get("createdLineId"):
        al = track("approval_line", {"lineId": sl["createdLineId"]},
                   f"아마란스 전자결재 > 결재선 관리에서 '{line_nm}' 삭제")
        ok, note = undo_approval_line(mcp, al["ref"], marker)
        R.append(("PASS" if ok else "FAIL", "delete_approval_line", note))
        if ok:
            untrack(al)
    else:
        skip("delete_approval_line", "생성 실패")

    # 메일 — 수신자는 본인 고정(Mcp.call 이 to 지정을 차단한다)
    sent_at = datetime.now()
    subj = f"{marker} 라이브 점검 {sent_at.strftime('%Y%m%d-%H%M%S')}"
    sm = run(mcp, "send_mail", lambda d: (len(json.dumps(d)) > 10, "본인 앞 발송"),
             subject=subj, html="<p>inno-creed 라이브 점검. 자동 삭제됩니다.</p>")
    if sm is not None:
        # sentAt 은 배달 대기를 얼마나 참을지 정하는 근거 — undo_mail 참조
        ml = track("mail",
                   {"subject": subj, "sentAt": sent_at.strftime("%Y-%m-%d %H:%M:%S")},
                   f"메일함에서 제목 '{subj}' 삭제")
        ok, note = undo_mail(mcp, ml["ref"], marker)
        R.append(("PASS" if ok else "FAIL", "delete_mail", note))
        if ok:
            untrack(ml)
    else:
        skip("delete_mail", "send_mail 실패")

    submit_scenario(mcp, fx, marker)

    for n, why in FORBIDDEN.items():
        skip(n, f"금지 — {why}")


def submit_scenario(mcp: Mcp, fx: dict, marker: str):
    """상신 → 즉시 회수. opt-in 전용.

    되돌리기는 완전하다(HP 근태 레코드까지 회수됨 — 2026-08-06 실측). 남는 위험은 **상신 실패**뿐이다:
    `create`는 성공했는데 `eap110A06`이 실패하면 취소할 eap 문서가 없어 HP 레코드가 고아로 남고,
    하드삭제 API가 없어 지울 수 없다. 그래서 전후로 `hp_records()`를 찍어 고아를 감지한다."""
    cfg = fx.get("submit")
    if not cfg:
        for n in SUBMIT_TOOLS:
            skip(n, "fixtures.json 에 submit 설정 없음")
        return
    if not submit_enabled():
        for n, why in SUBMIT_TOOLS.items():
            skip(n, f"opt-in 없음 — {why}")
        return

    # ① 사전 확인 — 상신하지 않고 병합 결재선을 본다. 예상과 다르면 상신하지 않는다.
    #    (회사가 양식 규칙을 바꿔 결재자·수신참조가 늘면 여기서 멈춘다.)
    pre = _probe_a03(cfg["formId"], cfg["lineId"])
    if pre is None:
        for n in SUBMIT_TOOLS:
            skip(n, "probe 바이너리 없음 — `cargo build --release --bin probe` 후 재시도")
        return
    want = cfg.get("expectedApprovers") or []
    if pre["approvers"] != want or len(pre["refer"]) != cfg.get("expectedRefer", 0) \
            or len(pre["oper"]) != cfg.get("expectedOper", 0):
        R.append(("FAIL", "submit_approval(결재선 사전확인)",
                  f"병합 결재선이 예상과 다름 — 상신하지 않음. "
                  f"결재={pre['approvers']} 참조={pre['refer']} 시행={pre['oper']} (예상 결재={want})"))
        skip("cancel_approval", "사전확인 불일치로 상신 생략")
        return
    R.append(("PASS", "submit_approval(결재선 사전확인)",
              f"결재 {len(pre['approvers'])}명 {pre['approvers']} · "
              f"참조 {len(pre['refer'])}건 {pre['refer']} · 시행 {len(pre['oper'])}건 {pre['oper']} · "
              f"{pre['formDTp']}"))

    # ② 아직 안 쓴 미래 평일을 고른다(중복근태 모달·기존 신청과 겹치지 않게).
    before = hp_records()
    used = set((before or {}).values())
    d, target = date.today() + timedelta(days=int(cfg.get("daysAhead", 90))), None
    for _ in range(60):
        if d.weekday() < 5 and d.strftime("%Y%m%d") not in used:
            target = d.strftime("%Y%m%d")
            break
        d += timedelta(days=1)
    if not target:
        for n in SUBMIT_TOOLS:
            skip(n, "미사용 미래 평일을 못 찾음")
        return

    # ③ 페이로드는 번들 가이드의 예시에서 날짜만 갈아끼운다.
    st, guide = mcp.call("get_approval_submission_guide", doc_type=cfg["docType"])[:2]
    if st == "ERR":
        for n in SUBMIT_TOOLS:
            skip(n, f"제출 가이드 조회 실패: {guide}")
        return
    dh = guide["guide"]["draftHelp"]
    hp, bd = json.loads(json.dumps(dh["hpApplicationExample"])), json.loads(json.dumps(dh["bindDataExample"]))
    note = f"{marker} 자동 점검 — 즉시 취소"
    for a in hp.get("applicationList", []):
        a.update(atDt=target, startDt=target, endDt=target, appRmkDc=note)
    try:
        iso = f"{target[:4]}-{target[4:6]}-{target[6:]}"
        it = bd["TABLE"]["dbTable1"]["group"][0]["group"][0]["items"]
        it.update(startDt=iso, endDt=iso, appRmkDc=note)
        if "taskDc" in it:
            it["taskDc"] = note
    except (KeyError, IndexError, TypeError):
        pass  # 양식마다 bindData 모양이 다르다 — 날짜 치환이 안 되면 예시 그대로 보낸다

    title = f"{marker} {cfg['docType']} {target} (즉시취소)"
    entry = None
    s = run(mcp, "submit_approval", lambda d: (int(d.get("docId", 0)) > 0,
                                               f"docId={d['docId']} 결재선 {d.get('lineCount')}명 · {target}"),
            form_id=cfg["formId"], doc_title=title, line_id=cfg["lineId"],
            hp_application_json=json.dumps(hp, ensure_ascii=False),
            bind_data_json=json.dumps(bd, ensure_ascii=False),
            doc_contents_html=f"<div>{target} — {note}</div>", numbering_id="")
    if s and s.get("docId"):
        entry = track("approval", {"docId": s["docId"], "formId": cfg["formId"]},
                      f"아마란스 전자결재에서 docId {s['docId']} 결재취소→상신취소→임시보관삭제")
        ok, msg = undo_approval(mcp, entry["ref"], marker)
        R.append(("PASS" if ok else "FAIL", "cancel_approval", msg))
        if ok:
            untrack(entry)
    else:
        skip("cancel_approval", "상신 실패 — 취소할 문서 없음")

    # ④ 고아 감지 — 상신이 실패했든 성공했든, 남은 HP 레코드가 있으면 지울 수 없다.
    after = hp_records()
    if before is not None and after is not None:
        orphan = {sq: dt for sq, dt in after.items() if sq not in before}
        if orphan:
            R.append(("FAIL", "HP근태레코드_고아",
                      f"⚠️ 취소로 회수되지 않은 HP 신청 {orphan} — **하드삭제 API가 없어 자동 정리 불가**. "
                      "아마란스 개인근태신청현황에서 직접 확인할 것"))
        else:
            R.append(("PASS", "HP근태레코드_고아", f"신규 고아 0건(전 {len(before)} → 후 {len(after)})"))


if __name__ == "__main__":
    main()
