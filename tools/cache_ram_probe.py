"""The parked-set bound of llama.cpp's RAM prompt cache
(docs/research/parallel-subagents.md §8, the caveat of its §3.2).

Every request is pinned to one slot (`id_slot`), so each new conversation
evicts the previous one from the slot into the RAM prompt cache
(`--cache-ram`, 8192 MiB by default), the way a `-np 1` server rotates the
runs of a turn. Conversations are added one at a time; after each addition
every earlier conversation is revisited with one short extra turn, and the
server's `timings.cache_n` says whether its context came back from the cache
(cache_n ~ its size) or had to be prefilled again (cache_n ~ 0). The first
addition after which an earlier conversation stops coming back is the bound.

Usage: python cache_ram_probe.py BASE_URL MAX_CONVERSATIONS [TOKENS_EACH]
"""
import json
import sys
import time

import requests

BASE = sys.argv[1].rstrip("/")
MAX_N = int(sys.argv[2])
TOKENS = int(sys.argv[3]) if len(sys.argv) > 3 else 12000
SLOT = 0
PARAGRAPH = ("Paragraph {i} of the {tag} archive describes a lighthouse keeper's ordinary "
             "evening: the lamp is lit, the log is written, the tide is noted.")
# ~30 tokens per paragraph on Qwen's tokenizer (measured in the admission smoke).
PARAGRAPHS = TOKENS // 30


def archive(tag):
    return "\n".join(PARAGRAPH.format(i=i, tag=tag) for i in range(PARAGRAPHS))


class Conversation:
    def __init__(self, idx):
        self.tag = f"conv{idx}"
        self.messages = [
            {"role": "system", "content": f"You are conversation {self.tag}. Answer in one word."},
            {"role": "user", "content": archive(self.tag) + "\n\nWhich archive is this? One word."},
        ]
        self.size = None  # the server's prompt_n after the first visit

    def visit(self, note):
        t0 = time.time()
        r = requests.post(
            f"{BASE}/v1/chat/completions",
            json={
                "model": "x",
                "messages": self.messages,
                "max_tokens": 8,
                "temperature": 0.0,
                "id_slot": SLOT,
                "cache_prompt": True,
                "chat_template_kwargs": {"enable_thinking": False},
            },
            timeout=600,
        )
        j = r.json()
        if "error" in j:
            print(json.dumps({"tag": self.tag, "note": note, "error": j["error"]}), flush=True)
            return None
        t = j.get("timings", {})
        reply = (j["choices"][0]["message"].get("content") or "").strip()
        rec = {
            "tag": self.tag,
            "note": note,
            "prompt_n": t.get("prompt_n"),
            "cache_n": t.get("cache_n"),
            "prompt_ms": round(t.get("prompt_ms", 0)),
            "wall_s": round(time.time() - t0, 2),
            "reply": reply[:20],
        }
        print(json.dumps(rec), flush=True)
        # Grow the history by one short exchange so the next visit is a
        # continuation, as a real turn would be.
        self.messages.append({"role": "assistant", "content": reply or "ok"})
        self.messages.append({"role": "user", "content": f"Again, one word ({note})."})
        if self.size is None:
            self.size = (rec["prompt_n"] or 0) + (rec["cache_n"] or 0)
        return rec


def main():
    props = requests.get(f"{BASE}/props").json()
    print(json.dumps({
        "model": props.get("model_alias"),
        "build": props.get("build_info"),
        "slots": props.get("total_slots"),
        "n_ctx": props["default_generation_settings"]["n_ctx"],
        "tokens_each": TOKENS,
        "paragraphs": PARAGRAPHS,
    }), flush=True)
    convs = []
    for k in range(MAX_N):
        c = Conversation(k)
        convs.append(c)
        c.visit(f"cold, as #{k + 1}")
        # Revisit every earlier conversation, oldest first: the one parked
        # longest is the first candidate for eviction.
        restored, evicted = [], []
        for earlier in convs[:-1]:
            rec = earlier.visit(f"revisit with {k + 1} parked")
            if rec is None:
                continue
            (restored if (rec["cache_n"] or 0) > earlier.size // 2 else evicted).append(earlier.tag)
        print(json.dumps({"parked": k + 1, "restored": restored, "evicted": evicted}), flush=True)
        if evicted:
            print(json.dumps({"bound": k, "note": f"with {k + 1} conversations parked, {len(evicted)} no longer come back"}), flush=True)


main()
