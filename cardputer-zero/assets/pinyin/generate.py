"""Convert pinned Rime Ice 8105 and base YAML word lists to Colibri TSV."""
import sys
from pathlib import Path


def main():
    entries = {}
    for filename, minimum in zip(sys.argv[1:], (1, 1000)):
        for line in Path(filename).read_text(encoding="utf-8").splitlines():
            fields = line.split("\t")
            if len(fields) != 3 or not fields[2].isdigit():
                continue
            text, pinyin, count = fields
            spelling = pinyin.replace(" ", "")
            count = int(count)
            if count < minimum or not spelling.isascii() or not spelling.isalpha() or not spelling.islower():
                continue
            key = (pinyin, text)
            entries[key] = max(count, entries.get(key, 0))
    for (spelling, text), count in sorted(entries.items(), key=lambda item: (item[0][0].replace(" ", ""), -item[1], item[0][1])):
        print(f"{spelling}\t{text}\t{count}")


if __name__ == "__main__":
    main()
