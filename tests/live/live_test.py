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
    "clock_in": "실제 근태 punch — 되돌릴 수 없다",
    "clock_out": "실제 근태 punch — 되돌릴 수 없다",
    "submit_approval": "실제 결재 상신 — 결재선의 사람들에게 알림이 간다",
    "cancel_approval": "상신 문서 회수 — 상신을 안 하므로 대상이 없다",
    "delete_temp_approval": "임시보관 문서 삭제 — 사용자의 진짜 초안을 지울 수 있다",
}


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
        if name in FORBIDDEN:
            # ② 물리 차단. 이 예외는 잡지 않는다 — 테스트를 즉시 죽이는 것이 의도다.
            raise AssertionError(f"금지 도구 호출 시도: {name} ({FORBIDDEN[name]})")
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
    r = mcp.call("delete_event", sch_seq=str(ref["schSeq"]), date=ref["date"])
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


def undo_mail(mcp: Mcp, ref: dict, marker: str):
    got = mcp.call("list_inbox")
    if got[0] == "ERR":
        return False, got[1]
    hit = next((m for m in got[1]["Records"]
                if ref["subject"] in str(m.get("subject", ""))), None)
    if hit is None:
        return True, "받은메일함에 없음(이미 정리됐거나 아직 도착 전)"
    if marker not in str(hit.get("subject", "")):
        return False, "마커 없는 메일이라 건드리지 않음"
    r = mcp.call("delete_mail", uids=str(hit["muid"]))
    return (r[0] != "ERR"), f"muid={hit['muid']} 휴지통 이동" if r[0] != "ERR" else r[1]


UNDO = {
    "reservation": undo_reservation,
    "event": undo_event,
    "approval_line": undo_approval_line,
    "mail": undo_mail,
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
    inbox = run(mcp, "list_inbox", lambda d: (isinstance(d["Records"], list), f"{len(d['Records'])}통"))
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
    run(mcp, "list_submission_guides", lambda d: (len(json.dumps(d)) > 100, "목록 반환"))
    run(mcp, "get_submission_guide", lambda d: (
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
            al = run(mcp, "list_attachments", lambda d: (
                len(d["files"]) > 0, f"{tgt['title'][:18]} · {len(d['files'])}개"),
                art_seq_no=str(tgt["artSeqNo"]), uid=str(tgt["attachmentUid"]))
            if al and al.get("files"):
                run(mcp, "download_attachment", lambda d: (
                    d["ok"] and d["bytes"] > 0, f"{d['serverFileName']} {d['bytes']}B"),
                    art_seq_no=str(tgt["artSeqNo"]), uid=str(tgt["attachmentUid"]),
                    file_sn=0, out_path=os.path.join(OUTDIR, "board_att.bin"))
            else:
                skip("download_attachment", "첨부 목록이 비어 대상 없음")
        else:
            skip("list_attachments", "첨부 있는 게시글 없음")
            skip("download_attachment", "첨부 있는 게시글 없음")
    else:
        for n in ("read_notice", "list_attachments", "download_attachment"):
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
        for n in ("create_event", "update_event", "delete_event"):
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

        e = run(mcp, "create_event", lambda d: (
            d["verified_by_readback"] and "개인캘린더" in d["calendar"],
            f"schSeq={d['schSeq']} {d['calendar']}"),
            title=title, start=past + "1000", end=past + "1100", contents="자동 점검 — 즉시 삭제됨")
        if e:
            ev = track("event", {"schSeq": e["schSeq"], "date": past},
                       f"아마란스 캘린더 {past} '{title}' 삭제")
            run(mcp, "update_event", lambda d: (
                str(d["schSeq"]) == str(e["schSeq"]) and d["title"] == title + "(수정)",
                "schSeq 유지(in-place)"),
                sch_seq=e["schSeq"], date=past, title=title + "(수정)")
            ok, note = undo_event(mcp, ev["ref"], marker)
            R.append(("PASS" if ok else "FAIL", "delete_event", note))
            if ok:
                untrack(ev)
        else:
            skip("update_event", "등록 실패")
            skip("delete_event", "등록 실패")

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
    subj = f"{marker} 라이브 점검 {datetime.now().strftime('%Y%m%d-%H%M%S')}"
    sm = run(mcp, "send_mail", lambda d: (len(json.dumps(d)) > 10, "본인 앞 발송"),
             subject=subj, html="<p>inno-creed 라이브 점검. 자동 삭제됩니다.</p>")
    if sm is not None:
        ml = track("mail", {"subject": subj}, f"메일함에서 제목 '{subj}' 삭제")
        ok, note = undo_mail(mcp, ml["ref"], marker)
        R.append(("PASS" if ok else "FAIL", "delete_mail", note))
        if ok:
            untrack(ml)
    else:
        skip("delete_mail", "send_mail 실패")

    for n, why in FORBIDDEN.items():
        skip(n, f"금지 — {why}")


if __name__ == "__main__":
    main()
