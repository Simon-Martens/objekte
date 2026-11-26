#!/usr/bin/env python3
"""Generate explicit jq predicates for the fixed 300-case Musenalm workload.

The predicates operate on source JSON and file metadata. This generator does not
use the search core's parser, binder, evaluator, or indexed results.
"""

from pathlib import Path
import json


OUTPUT = Path(__file__).with_name("oracle_musenalm.jq")
conditions: dict[int, str] = {}


def put(number: int, expression: str) -> None:
    assert number not in conditions, number
    conditions[number] = expression


def q(value: object) -> str:
    return json.dumps(value, ensure_ascii=False)


def field(name: str) -> str:
    return "$r." + name


def numeric(name: str, operator: str, value: int | float) -> str:
    ref = field(name)
    return f"(({ref} | type) == \"number\" and {ref} {operator} {value})"


def scalar(name: str, value: object) -> str:
    return f"{field(name)} == {q(value)}"


def text(name: str, needle: str, mode: str = "contains") -> str:
    ref = field(name)
    if mode == "contains":
        return f"str_has({ref}; {q(needle)})"
    if mode == "not_contains":
        return f"str_lacks({ref}; {q(needle)})"
    if mode == "lower":
        return f"str_lower_has({ref}; {q(needle)})"
    if mode == "folded":
        return f"str_lower_has({ref}; {q(needle)})"
    if mode == "prefix":
        return f"str_prefix({ref}; {q(needle)})"
    raise ValueError(mode)


def values(path: str, operator: str, target: object) -> str:
    parts = q(path.split("."))
    if operator == "eq":
        predicate = f". == {q(target)}"
    elif operator == "neq":
        predicate = f"type == \"string\" and . != {q(target)}"
    elif operator == "contains":
        predicate = f"type == \"string\" and contains({q(target)})"
    else:
        raise ValueError(operator)
    return f"any(vals($r; {parts}); {predicate})"


def length(path: str, operator: str, target: int) -> str:
    ref = field(path)
    return f"(($r | has({q(path)})) and ({ref} | type) == \"array\" and ({ref} | length) {operator} {target})"


# Text search is raw substring membership among JSON string leaves.
for number, collection, needle in (
    (1, "*", "Almanach"), (2, "*", "Taschenbuch"), (3, "*", "Leipzig"),
    (4, "*", "Wien"), (5, "*", "Goethe"), (6, "*", "ü"), (7, "*", "a"),
    (8, "*", "__objekte_missing_7c9f31__"),
    (9, "places", "Leipzig"), (10, "places", "Wien"),
    (11, "agents", "Goethe"), (12, "agents", "Schiller"),
    (13, "agents", "Müller"), (14, "agents", "Text"),
    (15, "agents", "https://"), (16, "series", "Almanach"),
    (17, "series", "Taschenbuch"), (18, "entries", "Vergnügen"),
    (19, "entries", "Almanach"), (20, "entries", "1808"),
    (21, "entries", "WIEN"), (22, "entries", "unbezeichnet"),
    (23, "contents", "Liebe"), (24, "contents", "Frühling"),
    (25, "contents", "Lied"), (26, "contents", "Der"),
    (27, "contents", "1808"), (28, "items", "Theodor Springmann Stiftung"),
    (29, "pages", "Musenalm"),
):
    put(number, f"text_has($texts; {q(needle)})" if collection == "*" else f"$collection == {q(collection)} and text_has($texts; {q(needle)})")
put(30, '$collection == "entries" and str_has($r.preferred_title; "Taschenbuch")')


for number, collection, expression in (
    (31, "places", scalar("fictional", False)),
    (32, "places", scalar("fictional", True)),
    (33, "places", scalar("edit_state", "Edited")),
    (34, "places", '$id == "d7kbujq7399g22j"'),
    (35, "places", numeric("musenalm_id", ">", 100)),
    (36, "places", numeric("musenalm_id", "<=", 100)),
    (37, "agents", scalar("corporate_body", True)),
    (38, "agents", scalar("corporate_body", False)),
    (39, "agents", scalar("fictional", True)),
    (40, "agents", scalar("fictional", False)),
    (41, "agents", scalar("edit_state", "Review")),
    (42, "agents", scalar("profession", "Text")),
    (43, "agents", numeric("musenalm_id", ">=", 1000)),
    (44, "entries", numeric("year", "==", 1808)),
    (45, "entries", numeric("year", "<", 1800)),
    (46, "entries", numeric("year", ">=", 1800) + " and " + numeric("year", "<", 1850)),
    (47, "entries", '$id == "00fx8ln651na0e3"'),
    (48, "entries", scalar("edit_state", "Edited")),
    (49, "entries", scalar("pseudonym", False)),
    (50, "contents", scalar("entry", "00fx8ln651na0e3")),
    (51, "contents", numeric("year", "==", 1808)),
    (52, "contents", numeric("year", ">=", 1800) + " and " + numeric("year", "<", 1850)),
    (53, "contents", numeric("numbering", "==", 1)),
    (54, "contents", numeric("numbering", ">", 1) + " and " + numeric("numbering", "<", 2)),
    (55, "contents", numeric("numbering", ">=", 10)),
    (56, "contents", scalar("pseudonym", True)),
    (57, "items", scalar("basis", True)),
    (58, "items", scalar("public", True)),
    (59, "items", scalar("owner", "Theodor Springmann Stiftung")),
    (60, "series", scalar("frequency", "jährlich")),
):
    put(number, f"$collection == {q(collection)} and ({expression})")


for number, collection, name, needle, mode in (
    (61, "places", "name", "Leipzig", "equal"),
    (62, "places", "name", "zig", "contains"),
    (63, "places", "name", "LEIPZIG", "lower"),
    (64, "places", "name", "Leip", "prefix"),
    (65, "places", "name", "zig", "not_contains"),
    (66, "agents", "name", "Goethe", "contains"),
    (67, "agents", "name", "goethe", "lower"),
    (68, "agents", "name", "Goethe", "contains"),
    (69, "agents", "pseudonyms", "", "contains"),
    (70, "agents", "name", "GOETHE", "folded"),
    (71, "entries", "preferred_title", "Almanach", "contains"),
    (72, "entries", "preferred_title", "TASCHENBUCH", "lower"),
    (73, "entries", "title_statement", "WIEN", "contains"),
    (74, "entries", "annotation", "Beschädigung", "not_contains"),
    (75, "entries", "place_statement", "Wien", "contains"),
    (76, "contents", "preferred_title", "Liebe", "contains"),
    (77, "contents", "preferred_title", "FRÜHLING", "lower"),
    (78, "contents", "incipit_statement", "Der", "contains"),
    (79, "series", "title", "Taschenbuch", "contains"),
):
    expression = scalar(name, needle) if mode == "equal" else text(name, needle, mode)
    put(number, f"$collection == {q(collection)} and ({expression})")
put(80, '$collection == "pages" and has_path($r; ["data", "description"])')


for number, collection, path, operator, target in (
    (81, "entries", "places", "eq", "d7kbujq7399g22j"),
    (82, "entries", "places", "neq", "d7kbujq7399g22j"),
    (84, "entries", "language", "eq", "ger"),
    (86, "entries", "carrier_type", "eq", "Band"),
    (87, "entries", "content_type", "eq", "Text"),
    (88, "entries", "agents.agent", "eq", "lngx6eyfw5qrzas"),
    (89, "entries", "agents.type", "eq", "Verlag"),
    (90, "entries", "agents.uncertain", "eq", True),
    (91, "entries", "items.owner", "eq", "Theodor Springmann Stiftung"),
    (92, "entries", "items.public", "eq", True),
    (93, "entries", "items.media", "eq", "Original"),
    (94, "entries", "musenalm_deprecated.status", "eq", "Original vorhanden"),
    (95, "contents", "contributors.agents", "eq", "lngx6eyfw5qrzas"),
    (96, "contents", "contributors.type", "eq", "Autor:in"),
    (97, "contents", "contributors.conjecture", "eq", True),
    (98, "contents", "contributors.uncertain", "eq", True),
    (99, "contents", "musenalm_type", "contains", "Lied"),
    (100, "contents", "language", "eq", "ger"),
    (101, "series", "entries", "eq", "00fx8ln651na0e3"),
    (102, "series", "related_entries.entry", "eq", "00fx8ln651na0e3"),
    (103, "items", "media", "eq", "Original"),
    (104, "agents", "data.gnd.gender.label", "eq", "Weiblich"),
    (105, "agents", "data.gnd.type", "contains", "Person"),
):
    put(number, f"$collection == {q(collection)} and {values(path, operator, target)}")
put(83, '$collection == "entries" and (' + values("places", "eq", "d7kbujq7399g22j") + ' | not)')
put(85, '$collection == "entries" and ($r | has("language")) and all($r.language[]?; . == "ger")')


for number, collection, expression in (
    (106, "agents", '($r | has("data")) and $r.data == null'),
    (107, "agents", '($r | has("data")) and $r.data != null'),
    (108, "agents", 'has_path($r; ["data", "gnd", "preferredName"])'),
    (109, "agents", '$file.size >= 4096'),
    (110, "entries", '($r | has("edit_fielddata")) and $r.edit_fielddata == null'),
    (111, "entries", "true"),
    (112, "entries", '($r | has("musenalm_deprecated")) and $r.musenalm_deprecated == null'),
    (113, "entries", length("places", "==", 0)),
    (114, "entries", length("places", ">", 0)),
    (115, "entries", length("items", ">=", 2)),
    (116, "contents", length("contributors", ">", 0)),
    (117, "contents", length("contributors", "==", 0)),
    (118, "contents", length("contributors", ">=", 3)),
    (119, "contents", '$file.modified_seconds >= 1767225600'),
    (120, "contents", scalar("incipit_statement", "")),
    (121, "series", length("entries", ">=", 3)),
    (122, "series", length("related_entries", ">", 0)),
    (123, "items", length("media", ">", 0)),
    (124, "items", '$file.birth_seconds == null'),
    (125, "pages", "true"),
):
    put(number, f"$collection == {q(collection)} and ({expression})")


# Sorting, pagination and return mode do not alter complete membership sets.
for number, original in (
    (126, 31), (127, 31), (128, 38), (129, 38), (130, 38),
    (131, 46), (132, 46), (133, 50), (137, 50), (138, 50),
    (139, 50), (140, 121),
):
    put(number, conditions[original])
for number in (134, 135, 136):
    put(number, '$collection == "contents" and ' + numeric("numbering", ">=", 0))


for case, collection, needle, predicate in (
    (141, "entries", "Almanach", conditions[46]),
    (142, "entries", "Wien", conditions[44]),
    (143, "entries", "Taschenbuch", conditions[84]),
    (144, "contents", "Liebe", conditions[52]),
    (145, "contents", "Lied", conditions[96]),
    (146, "agents", "Goethe", conditions[38]),
    (147, "agents", "Müller", conditions[107]),
    (148, "items", "Original", conditions[57]),
    (149, "series", "Almanach", conditions[121] + ' and str_has($file.relative_path; "data/series/")'),
    (150, "entries", "Almanach", '($r.edit_state == "Edited" or $r.edit_state == "Review") and ' + numeric("year", ">=", 1800) + ' and ' + length("places", ">", 0)),
):
    put(case, f"$collection == {q(collection)} and text_has($texts; {q(needle)}) and ({predicate})")


for number, collection, predicate in (
    (221, 'contents', '((str_has($r.preferred_title; "Frühling")))'),
    (222, 'contents', '(((($r.year | type) == "number" and $r.year >= 1800) and str_has($r.preferred_title; "Frühling")))'),
    (223, 'contents', '((str_has($r.preferred_title; "Frühling") and (($r.year | type) == "number" and $r.year >= 1800)))'),
    (224, 'contents', '(((($r.year | type) == "number" and $r.year < 1800) and str_has($r.preferred_title; "Frühling")))'),
    (225, 'contents', '(((($r.year | type) == "number" and $r.year >= 1820) and str_has($r.incipit_statement; "Der")))'),
    (226, 'contents', '((str_has($r.incipit_statement; "Liebe") and (($r.year | type) == "number" and $r.year < 1820)))'),
    (227, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Gedicht/Lied") and str_has($r.preferred_title; "Liebe")))'),
    (228, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Graphik") and (($r.year | type) == "number" and $r.year < 1800)))'),
    (229, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Prosa") and (($r.year | type) == "number" and $r.year >= 1820)))'),
    (230, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Rätsel") and (($r.contributors | type) == "array" and ($r.contributors | length) > 0)))'),
    (231, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Tabelle") and (($r.numbering | type) == "number" and $r.numbering <= 10)))'),
    (232, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Titel") and $r.numbering == 1))'),
    (233, 'contents', '((any(vals($r; ["musenalm_type"]); str_has(.; "Lied")) and str_has($r.responsibility_statement; "Rückert")))'),
    (234, 'contents', '((any(vals($r; ["contributors", "type"]); . == "Autor:in") and $r.responsibility_statement == "unbezeichnet"))'),
    (235, 'contents', '(((($r.contributors | type) == "array" and ($r.contributors | length) == 0) and str_has($r.preferred_title; "Lied")))'),
    (236, 'contents', '(((($r.scans | type) == "array" and ($r.scans | length) > 0) and str_has($r.preferred_title; "Winter")))'),
    (237, 'contents', '(((($r.scans | type) == "array" and ($r.scans | length) == 0) and str_has($r.incipit_statement; "Sommer")))'),
    (238, 'contents', '((any(vals($r; ["musenalm_pagination"]); str_has(.; "Arabische")) and (($r.numbering | type) == "number" and $r.numbering < 5)))'),
    (239, 'contents', '(($r.pseudonym == true and (($r.year | type) == "number" and $r.year >= 1800)))'),
    (240, 'contents', '(($r.entry == "00fx8ln651na0e3" and (($r.numbering | type) == "number" and $r.numbering <= 10)))'),
    (241, 'contents', '(($r.entry == "00fx8ln651na0e3" and (($r.contributors | type) == "array" and ($r.contributors | length) > 0)))'),
    (242, 'contents', '(((($r.numbering | type) == "number" and $r.numbering > 1) and (($r.numbering | type) == "number" and $r.numbering < 2)))'),
    (243, 'contents', '((str_has($r.preferred_title; "Mond")) or (str_has($r.incipit_statement; "Mond")))'),
    (244, 'contents', '((str_has($r.preferred_title; "Nacht") and str_lacks($r.incipit_statement; "Nacht")))'),
    (245, 'contents', '((str_lower_has($r.responsibility_statement; "goethe")))'),
    (246, 'contents', 'text_has($texts; "Lied") and (((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1810)))'),
    (247, 'contents', 'str_has($r.preferred_title; "Lied") and (((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1810)))'),
    (248, 'contents', 'str_has($r.incipit_statement; "Lied") and (((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1810)))'),
    (249, 'contents', 'text_has($texts; "Berlin") and ((any(vals($r; ["musenalm_type"]); . == "Graphik")))'),
    (250, 'contents', 'text_has($texts; "Schiller") and (((($r.contributors | type) == "array" and ($r.contributors | length) > 0)))'),
    (251, 'contents', 'text_has($texts; "Frühling") and ((any(vals($r; ["musenalm_type"]); . == "Gedicht/Lied")))'),
    (252, 'contents', 'text_has($texts; "Liebe") and (($r.year == 1808))'),
    (253, 'contents', '((str_has($r.preferred_title; "Lied")))'),
    (254, 'contents', '(((($r.year | type) == "number" and $r.year >= 1820) and (($r.year | type) == "number" and $r.year < 1825)))'),
    (255, 'contents', '((str_has($r.responsibility_statement; "Mayer") and any(vals($r; ["contributors", "type"]); . == "Autor:in")))'),
    (256, 'entries', '((str_has($r.preferred_title; "Kalender")))'),
    (257, 'entries', '(((($r.year | type) == "number" and $r.year >= 1800) and str_has($r.preferred_title; "Kalender")))'),
    (258, 'entries', '((str_has($r.preferred_title; "Kalender") and (($r.year | type) == "number" and $r.year >= 1800)))'),
    (259, 'entries', '(((($r.year | type) == "number" and $r.year < 1800) and str_has($r.preferred_title; "Taschenbuch")))'),
    (260, 'entries', '((str_has($r.place_statement; "Leipzig") and str_has($r.preferred_title; "Taschenbuch")))'),
    (261, 'entries', '((str_has($r.place_statement; "Berlin") and (($r.year | type) == "number" and $r.year < 1800)))'),
    (262, 'entries', '((str_has($r.place_statement; "Göttingen") and (($r.year | type) == "number" and $r.year >= 1800)))'),
    (263, 'entries', '((str_has($r.place_statement; "Cotta")) or (str_has($r.publication_statement; "Cotta")))'),
    (264, 'entries', '((str_lower_has($r.place_statement; "wien") and str_lower_has($r.preferred_title; "almanach")))'),
    (265, 'entries', '((any(vals($r; ["agents", "type"]); . == "Verlag") and (($r.agents | type) == "array" and ($r.agents | length) >= 2)))'),
    (266, 'entries', '((any(vals($r; ["agents", "type"]); . == "Herausgeber:in") and (($r.places | type) == "array" and ($r.places | length) > 0)))'),
    (267, 'entries', '((any(vals($r; ["agents", "type"]); . == "Druck")) or (any(vals($r; ["agents", "type"]); . == "Verlag")))'),
    (268, 'entries', '((any(vals($r; ["items", "media"]); . == "Original") and (($r.year | type) == "number" and $r.year < 1800)))'),
    (269, 'entries', '((any(vals($r; ["items", "media"]); . == "Reproduktion") and any(vals($r; ["language"]); . == "ger")))'),
    (270, 'entries', '((any(vals($r; ["items", "public"]); . == true)) or (any(vals($r; ["items", "media"]); . == "Reproduktion")))'),
    (271, 'entries', '((any(vals($r; ["language"]); . == "fre")) or (any(vals($r; ["language"]); . == "ger")))'),
    (272, 'entries', '((any(vals($r; ["language"]); . == "ger") and any(vals($r; ["content_type"]); . == "unbewegtes Bild")))'),
    (273, 'entries', '((any(vals($r; ["carrier_type"]); . == "Band") and (($r.agents | type) == "array" and ($r.agents | length) == 0)))'),
    (274, 'entries', '(((($r.places | type) == "array" and ($r.places | length) == 0) and str_has($r.preferred_title; "Kalender")))'),
    (275, 'entries', '(((($r.places | type) == "array" and ($r.places | length) > 0) and str_has($r.place_statement; "Gotha")))'),
    (276, 'entries', '(((($r.items | type) == "array" and ($r.items | length) >= 2) and $r.edit_state == "Edited"))'),
    (277, 'entries', '(($r.edit_state == "Review") or ($r.edit_state == "Unknown"))'),
    (278, 'entries', '(($r.edit_state == "Seen" and (($r.year | type) == "number" and $r.year < 1820)))'),
    (279, 'entries', '((str_has($r.preferred_title; "Almanach")))'),
    (280, 'entries', '((str_has($r.preferred_title; "Almanach")))'),
    (281, 'entries', '(($r.year == 1808))'),
    (282, 'entries', 'text_has($texts; "Taschenbuch") and (((($r.year | type) == "number" and $r.year >= 1790) and (($r.year | type) == "number" and $r.year < 1800)))'),
    (283, 'entries', 'str_has($r.preferred_title; "Taschenbuch") and (((($r.year | type) == "number" and $r.year >= 1790) and (($r.year | type) == "number" and $r.year < 1800)))'),
    (284, 'entries', 'text_has($texts; "Leipzig") and ((any(vals($r; ["agents", "type"]); . == "Verlag")))'),
    (285, 'entries', 'str_has($r.place_statement; "Wien") and (((($r.items | type) == "array" and ($r.items | length) > 0)))'),
    (286, 'entries', 'text_has($texts; "Berlin") and (($r.edit_state == "Edited"))'),
    (287, 'entries', '((any(vals($r; ["musenalm_deprecated", "status"]); . == "Original vorhanden") and (($r.items | type) == "array" and ($r.items | length) > 0)))'),
    (288, 'entries', '((any(vals($r; ["places"]); . == "d7kbujq7399g22j") and (($r.year | type) == "number" and $r.year < 1850)))'),
    (289, 'agents', '(($r.profession == "Graphik"))'),
    (290, 'agents', '(($r.profession == "Musik" and $r.corporate_body == false))'),
    (291, 'agents', '((str_has($r.profession; "Text") and str_has($r.name; "Friedrich")))'),
    (292, 'agents', '((str_has($r.profession; "Graphik") and str_has($r.name; "Johann")))'),
    (293, 'agents', '((str_has($r.name; "Wilhelm") and $r.edit_state == "Edited"))'),
    (294, 'agents', '((str_lower_has($r.name; "müller") and $r.corporate_body == false))'),
    (295, 'agents', '((str_has($r.name; "Schiller")) or (str_has($r.name; "Goethe")))'),
    (296, 'agents', '(($r.profession == "Text") or ($r.profession == "Musik"))'),
    (297, 'agents', '(($r.corporate_body == true and str_has($r.name; "Verlag")))'),
    (298, 'agents', '((any(vals($r; ["data", "gnd", "gender", "label"]); . == "Weiblich") and str_has($r.profession; "Text")))'),
    (299, 'agents', '((any(vals($r; ["data", "gnd", "gender", "label"]); . == "Männlich") and $r.profession == "Graphik"))'),
    (300, 'agents', '((any(vals($r; ["data", "gnd", "type"]); str_has(.; "Person")) and str_has($r.name; "Johann")))'),
    (301, 'agents', '((str_has($r.uri; "d-nb.info") and $r.profession == "Musik"))'),
    (302, 'agents', '((str_has($r.biographical_data; "Jh.") and $r.profession == "Graphik"))'),
    (303, 'agents', '((str_has($r.pseudonyms; "von") and $r.corporate_body == false))'),
    (304, 'agents', '(($r.edit_state == "Review"))'),
    (305, 'agents', '(($r.profession == "Text"))'),
    (306, 'agents', 'text_has($texts; "Berlin") and (($r.profession == "Graphik"))'),
    (307, 'agents', 'str_has($r.name; "Friedrich") and (($r.corporate_body == false))'),
    (308, 'agents', 'text_has($texts; "Goethe") and ((str_has($r.profession; "Text")))'),
    (309, 'items', '((any(vals($r; ["media"]); . == "Reproduktion")))'),
    (310, 'items', '((any(vals($r; ["media"]); . == "Original") and $r.condition == "3b"))'),
    (311, 'items', '((any(vals($r; ["media"]); . == "Original") and str_has($r.location; "105B")))'),
    (312, 'items', '(($r.condition == "4b") or ($r.condition == "5b"))'),
    (313, 'items', '(($r.condition == "3b" and $r.basis == true))'),
    (314, 'items', '((str_has($r.owner; "Springmann") and str_has($r.location; "105H")))'),
    (315, 'items', '(((($r.scans | type) == "array" and ($r.scans | length) > 0) and $r.basis == true))'),
    (316, 'items', '(((($r.scans | type) == "array" and ($r.scans | length) == 0) and $r.location == "105B04"))'),
    (317, 'items', '(((($r.media | type) == "array" and ($r.media | length) > 0) and $r.condition == "1b"))'),
    (318, 'items', '((str_has($r.identifier; "405") and any(vals($r; ["media"]); . == "Original")))'),
    (319, 'items', 'text_has($texts; "105B04") and (($r.basis == true))'),
    (320, 'items', 'str_has($r.owner; "Springmann") and ((any(vals($r; ["media"]); . == "Original")))'),
    (321, 'items', '((str_has($r.location; "105")))'),
    (322, 'items', '((str_has($r.owner; "Springmann")))'),
    (323, 'series', '((str_has($r.title; "Kalender")))'),
    (324, 'series', '((str_lower_has($r.title; "taschenbuch") and $r.frequency == "jährlich"))'),
    (325, 'series', '((str_has($r.title; "Almanach") and (($r.entries | type) == "array" and ($r.entries | length) >= 3)))'),
    (326, 'series', '(((($r.entries | type) == "array" and ($r.entries | length) == 1) and str_has($r.title; "Kalender")))'),
    (327, 'series', '(((($r.entries | type) == "array" and ($r.entries | length) >= 10) and $r.frequency == "jährlich"))'),
    (328, 'series', '(((($r.related_entries | type) == "array" and ($r.related_entries | length) > 0) and (($r.entries | type) == "array" and ($r.entries | length) > 0)))'),
    (329, 'series', '((str_has($r.refs; "K") and str_has($r.title; "Taschenbuch")))'),
    (330, 'series', '((str_has($r.title; "Almanach")) or (str_has($r.title; "Taschenbuch")))'),
    (331, 'series', 'text_has($texts; "Berlin") and (($r.frequency == "jährlich"))'),
    (332, 'series', 'str_has($r.title; "Kalender") and (((($r.entries | type) == "array" and ($r.entries | length) >= 3)))'),
    (333, 'series', '((str_has($r.title; "Taschenbuch")))'),
    (334, 'series', '(((($r.entries | type) == "array" and ($r.entries | length) >= 5)))'),
    (335, 'places', '(($r.name == "Berlin"))'),
    (336, 'places', '(($r.fictional == false and str_has($r.name; "burg")))'),
    (337, 'places', '((str_lower_has($r.name; "berg")))'),
    (338, 'places', '((str_has($r.name; "Frankfurt")) or (str_has($r.name; "Main")))'),
    (339, 'places', '(($r.edit_state == "Edited" and str_has($r.name; "Wien")))'),
    (340, 'places', '(($r.fictional == true))'),
    (341, 'places', '((str_has($r.name; "Leipzig") and (($r.musenalm_id | type) == "number" and $r.musenalm_id > 0)))'),
    (342, 'places', '((str_has($r.pseudonyms; "Wien")) or (str_has($r.name; "Wien")))'),
    (343, 'places', 'str_has($r.name; "Berlin") and (($r.fictional == false))'),
    (344, 'places', '(((($r.musenalm_id | type) == "number" and $r.musenalm_id <= 100)))'),
    (345, 'pages', '(($r.key == "kontakt"))'),
    (346, 'pages', '((str_has($r.url; "/redaktion/")))'),
    (347, 'pages', '((str_has($r.title; "Kontakt")) or (str_has($r.url; "literatur")))'),
    (348, 'pages', '((str_has($r.url; "/redaktion/")))'),
    (349, 'contents', 'str_has($r.preferred_title; "Sommer")'),
    (350, 'contents', 'str_has($r.incipit_statement; "Sommer")'),
    (351, 'contents', 'text_has($texts; "Sommer")'),
    (352, 'contents', 'text_has($texts; "Sommer") and (((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1820)))'),
    (353, 'contents', 'str_has($r.preferred_title; "Sommer") and (((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1820)))'),
    (354, 'contents', '(((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1820) and str_has($r.preferred_title; "Sommer")))'),
    (355, 'contents', '((str_has($r.preferred_title; "Sommer") and (($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1820)))'),
    (356, 'contents', '(((($r.scans | type) == "array" and ($r.scans | length) > 0) and str_has($r.preferred_title; "Sommer")))'),
    (357, 'contents', '(((($r.contributors | type) == "array" and ($r.contributors | length) == 0) and str_has($r.preferred_title; "Sommer")))'),
    (358, 'contents', '((str_has($r.preferred_title; "Sommer")) or (str_has($r.preferred_title; "Winter")))'),
    (359, 'contents', '((str_has($r.incipit_statement; "Frühling")) or (str_has($r.preferred_title; "Frühling")))'),
    (360, 'contents', '((any(vals($r; ["musenalm_type"]); . == "Rätsel")))'),
    (361, 'entries', '(((($r.year | type) == "number" and $r.year >= 1790) and (($r.year | type) == "number" and $r.year < 1800) and str_has($r.preferred_title; "Kalender")))'),
    (362, 'entries', '(((($r.year | type) == "number" and $r.year >= 1800) and (($r.year | type) == "number" and $r.year < 1810) and str_has($r.preferred_title; "Kalender")))'),
    (363, 'entries', 'text_has($texts; "Almanach") and (((($r.items | type) == "array" and ($r.items | length) > 0) and (($r.places | type) == "array" and ($r.places | length) == 0)))'),
    (364, 'agents', '((str_has($r.name; "Friedrich")))'),
    (365, 'agents', 'text_has($texts; "Berlin") and (($r.profession == "Musik"))'),
    (366, 'series', 'str_has($r.title; "Taschenbuch") and (((($r.related_entries | type) == "array" and ($r.related_entries | length) > 0)))'),
    (367, 'items', 'text_has($texts; "Springmann") and (($r.condition == "3b"))'),
    (368, 'pages', 'str_has($r.title; "Musenalm")'),
    (369, 'places', '((str_has($r.name; "Dresden")) or (str_has($r.name; "Leipzig")))'),
    (370, 'entries', 'str_has($r.preferred_title; "Almanach") and (((($r.year | type) == "number" and $r.year >= 1810) and (($r.year | type) == "number" and $r.year < 1820)))'),
):
    put(number, f"$collection == {q(collection)} and ({predicate})")

case_numbers = [*range(1, 151), *range(221, 371)]
assert sorted(conditions) == case_numbers
header = '''# Generated by generate_jq_oracle.py from explicit workload semantics.
# Expected memberships are evaluated directly from original JSON source files.
def str_has($value; $needle): ($value | type) == "string" and ($value | contains($needle));
def str_lacks($value; $needle): ($value | type) == "string" and (($value | contains($needle)) | not);
def str_prefix($value; $needle): ($value | type) == "string" and ($value | startswith($needle));
def fold($value): $value | gsub("Ü"; "ü") | ascii_downcase;
def str_lower_has($value; $needle): ($value | type) == "string" and (fold($value) | contains(fold($needle)));
def text_has($texts; $needle): any($texts[]; contains($needle));
def vals($value; $parts):
  if ($parts | length) == 0 then
    if ($value | type) == "array" then $value[] | vals(.; []) else $value end
  elif ($value | type) == "array" then $value[] | vals(.; $parts)
  elif ($value | type) == "object" and ($value | has($parts[0])) then $value[$parts[0]] | vals(.; $parts[1:])
  else empty end;
def has_path($value; $parts): any(vals($value; $parts); true);

inputs
| . as $r
| input_filename as $path
| ($path | split("/")[-1] | sub("\\\\.json$"; "")) as $id
| $metadata[0][$path] as $file
| [$r | .. | strings] as $texts
| [
'''
lines = [header]
for case in case_numbers:
    suffix = "," if case != case_numbers[-1] else ""
    lines.append(f'    (if {conditions[case]} then "Q{case:03}" else empty end){suffix}\n')
lines.append('  ][] as $case\n| {case: $case, collection: $collection, id: $id}\n')
OUTPUT.write_text("".join(lines))
print(f"wrote {OUTPUT}: {len(conditions)} explicit predicates")
