from pathlib import Path
import sys


data = Path(sys.argv[1]) / "data"
data.mkdir()
(data / "00.json").write_text(
    '{"title":"hit","text":"old","text":"new","n":1e0,"rating":5}'
)
(data / "01.json").write_text(
    '{"title":"hit","text":"old","text":null,"n":1e0,"rating":5}'
)
(data / "02.json").write_text('{"title":"miss","text":"new","n":1e0,"rating":0}')
(data / "03.json").write_text('{"title":"miss","text":"old","n":2,"rating":5}')
