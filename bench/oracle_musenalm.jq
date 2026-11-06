# These predicates are handwritten from the fixed workload's intended meaning.
# jq reads each original JSON file directly; no core parser, filter binder,
# posting, cache, or snapshot participates in the expected results.
def matches($q; $collection; $record; $id; $texts):
  if $q == "Q001" then any($texts[]; contains("Almanach"))
  elif $q == "Q008" then any($texts[]; contains("__objekte_missing_7c9f31__"))
  elif $q == "Q013" then $collection == "agents" and any($texts[]; contains("Müller"))
  elif $q == "Q024" then $collection == "contents" and any($texts[]; contains("Frühling"))
  elif $q == "Q031" then $collection == "places" and $record.fictional == false
  elif $q == "Q034" then $collection == "places" and $id == "d7kbujq7399g22j"
  elif $q == "Q045" then $collection == "entries" and (($record.year? | type) == "number") and $record.year < 1800
  elif $q == "Q050" then $collection == "contents" and $record.entry == "00fx8ln651na0e3"
  elif $q == "Q054" then $collection == "contents" and (($record.numbering? | type) == "number") and $record.numbering > 1 and $record.numbering < 2
  elif $q == "Q061" then $collection == "places" and $record.name == "Leipzig"
  elif $q == "Q062" then $collection == "places" and ($record.name? | strings | contains("zig"))
  elif $q == "Q065" then $collection == "places" and ($record.name? | strings | contains("zig") | not)
  elif $q == "Q074" then $collection == "entries" and ($record.annotation? | strings | contains("Beschädigung") | not)
  elif $q == "Q081" then $collection == "entries" and any($record.places[]?; . == "d7kbujq7399g22j")
  elif $q == "Q082" then $collection == "entries" and any($record.places[]?; type == "string" and . != "d7kbujq7399g22j")
  elif $q == "Q083" then $collection == "entries" and (any($record.places[]?; . == "d7kbujq7399g22j") | not)
  elif $q == "Q084" then $collection == "entries" and any($record.language[]?; . == "ger")
  elif $q == "Q085" then $collection == "entries" and ($record | has("language")) and all($record.language[]?; . == "ger")
  elif $q == "Q088" then $collection == "entries" and any($record.agents[]?; .agent == "lngx6eyfw5qrzas")
  elif $q == "Q093" then $collection == "entries" and any($record.items[]?; (.media? | if type == "array" then any(.[]; . == "Original") else . == "Original" end))
  elif $q == "Q099" then $collection == "contents" and any($record.musenalm_type[]?; type == "string" and contains("Lied"))
  elif $q == "Q106" then $collection == "agents" and ($record | has("data")) and $record.data == null
  elif $q == "Q107" then $collection == "agents" and ($record | has("data")) and $record.data != null
  elif $q == "Q113" then $collection == "entries" and ($record | has("places")) and (($record.places | type) == "array") and (($record.places | length) == 0)
  elif $q == "Q114" then $collection == "entries" and ($record | has("places")) and (($record.places | type) == "array") and (($record.places | length) > 0)
  elif $q == "Q115" then $collection == "entries" and ($record | has("items")) and (($record.items | type) == "array") and (($record.items | length) >= 2)
  elif $q == "Q117" then $collection == "contents" and ($record | has("contributors")) and (($record.contributors | type) == "array") and (($record.contributors | length) == 0)
  elif $q == "Q120" then $collection == "contents" and $record.incipit_statement == ""
  elif $q == "Q125" then $collection == "pages"
  elif $q == "Q141" then $collection == "entries" and any($texts[]; contains("Almanach")) and (($record.year? | type) == "number") and $record.year >= 1800 and $record.year < 1850
  elif $q == "Q145" then $collection == "contents" and any($texts[]; contains("Lied")) and any($record.contributors[]?; .type == "Autor:in")
  elif $q == "Q150" then $collection == "entries" and any($texts[]; contains("Almanach")) and (($record.edit_state == "Edited") or ($record.edit_state == "Review")) and (($record.year? | type) == "number") and $record.year >= 1800 and ($record | has("places")) and (($record.places | type) == "array") and (($record.places | length) > 0)
  else error("unknown oracle case: \($q)") end;

inputs
| . as $record
| (input_filename | split("/")[-1] | sub("\\.json$"; "")) as $id
| [$record | .. | strings] as $texts
| ["Q001", "Q008", "Q013", "Q024", "Q031", "Q034", "Q045", "Q050", "Q054",
   "Q061", "Q062", "Q065", "Q074", "Q081", "Q082", "Q083", "Q084", "Q085",
   "Q088", "Q093", "Q099", "Q106", "Q107", "Q113", "Q114", "Q115",
   "Q117", "Q120", "Q125", "Q141", "Q145", "Q150"][] as $case
| select(matches($case; $collection; $record; $id; $texts))
| {case: $case, collection: $collection, id: $id}
