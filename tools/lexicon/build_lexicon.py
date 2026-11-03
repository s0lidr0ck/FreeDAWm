#!/usr/bin/env python3
"""Builds the Lyrics tab's offline word data (assets/lexicon/*.lex) from the CMU Pronouncing
Dictionary and WordNet 3.1.

    python -I tools/lexicon/build_lexicon.py <cmudict.dict> <wordnet-3.1 dict dir> [out dir]

Inputs (download them yourself; not kept in the repo):
  - cmudict.dict: https://github.com/cmusphinx/cmudict (BSD-style licence)
  - WordNet 3.1:  https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz, its `dict/` folder
    (WordNet licence)
Output (default `assets/lexicon/`): `pron.lex` and `wordnet.lex`, each zlib-compressed UTF-8 text
(formats below), read by `crates/engine/src/lexicon/`. Deterministic: the same inputs give the
same bytes.

pron.lex, after the header line `FREEDAWM-PRON 1`, one line per word (sorted):
    word TAB pronunciation[,pronunciation...] TAB commonness
  pronunciation: ARPAbet phones separated by spaces (vowels carry stress 0/1/2), as cmudict;
  commonness: 0..9, from WordNet sense counts of the word or its base form (0 = not in WordNet).

wordnet.lex, after the header line `FREEDAWM-WORDNET 1`, three sections:
  `#SYNSETS`, one line per synset (its line number, from 0, is its id):
    pos TAB lemma|lemma... TAB definition TAB example|example... TAB pointers
    pos: n v a r (adjective satellites are a); pointers: space-separated `<kind><id>`,
    kind @ hypernym, ~ hyponym (at most 12), & similar to, ^ also see, ! antonym.
  `#INDEX`, one line per lemma (sorted): lemma TAB sense count total TAB id,id,... (most used first)
  `#EXC`, irregular forms: form TAB base[,base...]
"""

import math
import os
import re
import sys
import zlib

POS_FILES = [("noun", "n"), ("verb", "v"), ("adj", "a"), ("adv", "r")]
KEEP_PTRS = {"@": "@", "@i": "@", "~": "~", "~i": "~", "&": "&", "^": "^", "!": "!"}
MAX_HYPONYMS = 12


def read_cmudict(path):
    prons = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            parts = line.split()
            word = re.sub(r"\(\d+\)$", "", parts[0]).lower()
            phones = " ".join(parts[1:])
            if not phones or "\t" in word or "," in phones:
                continue
            lst = prons.setdefault(word, [])
            if phones not in lst:
                lst.append(phones)
    return prons


def read_wordnet(d):
    synsets = []  # (pos, lemmas, definition, examples, [(kind, (pos, offset))])
    where = {}  # (pos letter of file, offset) -> id
    for name, p in POS_FILES:
        with open(os.path.join(d, "data." + name), encoding="utf-8", errors="replace") as f:
            for line in f:
                if line.startswith("  "):
                    continue
                head, _, gloss = line.partition(" | ")
                t = head.split()
                offset, sstype = t[0], t[2]
                wcnt = int(t[3], 16)
                lemmas = []
                for i in range(wcnt):
                    w = t[4 + 2 * i]
                    w = re.sub(r"\((a|p|ip)\)$", "", w).replace("_", " ")
                    if w not in lemmas:
                        lemmas.append(w)
                k = 4 + 2 * wcnt
                pcnt = int(t[k])
                ptrs = []
                for i in range(pcnt):
                    sym, off, ppos = t[k + 1 + 4 * i], t[k + 2 + 4 * i], t[k + 3 + 4 * i]
                    if sym in KEEP_PTRS:
                        ppos = "a" if ppos == "s" else ppos
                        ptrs.append((KEEP_PTRS[sym], (ppos, off)))
                gloss = gloss.strip()
                examples = re.findall(r'"([^"]+)"', gloss)
                definition = re.split(r';\s*"', gloss, maxsplit=1)[0].strip().rstrip(";").strip()
                where[(p, offset)] = len(synsets)
                synsets.append(("a" if sstype == "s" else sstype, lemmas, definition, examples, ptrs))
    # Sense counts per (lemma, synset) from index.sense.
    counts = {}
    with open(os.path.join(d, "index.sense"), encoding="utf-8") as f:
        for line in f:
            t = line.split()
            if len(t) < 4:
                continue
            lemma = t[0].split("%", 1)[0].replace("_", " ")
            sstype = int(t[0].split("%", 1)[1].split(":", 1)[0])
            p = {1: "n", 2: "v", 3: "a", 4: "r", 5: "a"}[sstype]
            sid = where.get((p, t[1]))
            if sid is not None:
                counts[(lemma, sid)] = int(t[3])
    # The index: per lemma its synsets in each pos's own order (most used first).
    index = {}
    for name, p in POS_FILES:
        with open(os.path.join(d, "index." + name), encoding="utf-8") as f:
            for line in f:
                if line.startswith("  "):
                    continue
                t = line.split()
                lemma = t[0].replace("_", " ")
                sense_cnt = int(t[2])
                ptr_cnt = int(t[3])
                offs = t[4 + ptr_cnt + 2:][:sense_cnt]
                lst = index.setdefault(lemma, [])
                for off in offs:
                    sid = where.get((p, off))
                    if sid is not None and sid not in lst:
                        lst.append(sid)
    exc = {}
    for name, _ in POS_FILES:
        with open(os.path.join(d, name + ".exc"), encoding="utf-8") as f:
            for line in f:
                t = line.split()
                if len(t) >= 2:
                    form = t[0].replace("_", " ")
                    for base in t[1:]:
                        base = base.replace("_", " ")
                        lst = exc.setdefault(form, [])
                        if base not in lst:
                            lst.append(base)
    return synsets, where, counts, index, exc


def bases(word, exc):
    """Candidate base forms of an inflected word (WordNet's morphy rules, simplified)."""
    out = [word]
    out += exc.get(word, [])
    for suf, rep in [("ies", "y"), ("es", ""), ("s", ""), ("ves", "f"), ("ied", "y"), ("ed", ""), ("ed", "e"),
                     ("ing", ""), ("ing", "e"), ("er", ""), ("er", "e"), ("est", ""), ("est", "e"), ("ly", ""),
                     ("ier", "y"), ("iest", "y"), ("ily", "y")]:
        if word.endswith(suf) and len(word) > len(suf) + 1:
            stem = word[: -len(suf)] + rep
            out.append(stem)
            # Doubled final consonant: "running" -> "run".
            if rep == "" and len(stem) > 2 and stem[-1] == stem[-2] and stem[-1] not in "aeiouls":
                out.append(stem[:-1])
    return out


def clean(s):
    return s.replace("\t", " ").replace("|", "/").strip()


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    cmu, wn = sys.argv[1], sys.argv[2]
    here = os.path.dirname(os.path.abspath(__file__))
    out = sys.argv[3] if len(sys.argv) > 3 else os.path.join(here, "..", "..", "assets", "lexicon")
    os.makedirs(out, exist_ok=True)
    prons = read_cmudict(cmu)
    synsets, where, counts, index, exc = read_wordnet(wn)

    lemma_total = {}
    for (lemma, _sid), c in counts.items():
        key = lemma.lower()
        lemma_total[key] = lemma_total.get(key, 0) + c
    # Common words only: a lemma written in lower case in some synset (not only as a name:
    # "Guam", "John").
    in_wn = set()
    for _pos, lemmas, _d, _e, _p in synsets:
        for l in lemmas:
            if l == l.lower():
                in_wn.add(l)

    def commonness(word):
        best = 0
        for b in bases(word, exc):
            if b in in_wn:
                tc = lemma_total.get(b, 0)
                best = max(best, min(9, 1 + int(math.log2(1 + tc))))
        return best

    lines = ["FREEDAWM-PRON 1"]
    for word in sorted(prons):
        lines.append(f"{word}\t{','.join(prons[word])}\t{commonness(word)}")
    write(os.path.join(out, "pron.lex"), "\n".join(lines) + "\n")

    # Synset ids in file order; hyponym lists pruned.
    lines = ["FREEDAWM-WORDNET 1", "#SYNSETS"]
    for pos, lemmas, definition, examples, ptrs in synsets:
        ps = []
        hypo = 0
        for kind, target in ptrs:
            tid = where.get(target)
            if tid is None:
                continue
            if kind == "~":
                hypo += 1
                if hypo > MAX_HYPONYMS:
                    continue
            item = f"{kind}{tid}"
            if item not in ps:
                ps.append(item)
        lines.append("\t".join([pos, "|".join(clean(l) for l in lemmas), clean(definition),
                                "|".join(clean(e) for e in examples), " ".join(ps)]))
    lines.append("#INDEX")
    for lemma in sorted(index):
        sids = index[lemma]
        # Most used first (stable on each pos's own order).
        order = sorted(range(len(sids)), key=lambda i: (-counts.get((lemma, sids[i]), 0), i))
        sids = [sids[i] for i in order]
        total = sum(counts.get((lemma, s), 0) for s in sids)
        lines.append(f"{lemma}\t{total}\t{','.join(str(s) for s in sids)}")
    lines.append("#EXC")
    for form in sorted(exc):
        lines.append(f"{form}\t{','.join(exc[form])}")
    write(os.path.join(out, "wordnet.lex"), "\n".join(lines) + "\n")


def write(path, text):
    raw = text.encode("utf-8")
    z = zlib.compress(raw, 9)
    with open(path, "wb") as f:
        f.write(z)
    print(f"{path}: {len(raw):,} bytes of text, {len(z):,} compressed")


if __name__ == "__main__":
    main()
