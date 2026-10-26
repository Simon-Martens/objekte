# Completing JSON schema

Complete structured UI support for schema-defined data. Validation support alone does not mean the UI can represent or edit a schema construct appropriately.

- [ ] **Union types:** support choosing and editing alternatives declared with multiple `type` values, `oneOf`, or `anyOf`, including nullable fields. Preserve the distinction between exactly one matching branch (`oneOf`) and at least one (`anyOf`).
- [ ] **Mixed arrays:** support arrays whose items can have different types or shapes, with an appropriate editor and alternative selection for each item.
- [ ] **Tuples:** support position-specific schemas through `prefixItems`, respecting array length constraints and restrictions on additional items. Do not offer reordering when it would violate positional rules.
- [ ] **Conditional fields:** adapt forms to `if`/`then`/`else`, `dependentSchemas`, and `dependentRequired`. Update applicable controls and required indicators without silently deleting existing values; conditional requirements do not automatically imply hiding fields.
- [ ] **Schema composition:** combine `allOf` branches into a coherent form while applying every constraint. Treat composition as an intersection of rules, not an override mechanism, and explain incompatible constraints.
- [ ] **Dynamic-key objects:** offer key–value editing and an Add entry action for dictionaries governed by `additionalProperties`, `patternProperties`, and `propertyNames`. Validate keys and select the applicable value schema.
- [ ] **Constraint-aware controls:** display numeric bounds and `multipleOf` requirements, string length limits and character counts, and useful `pattern` guidance. Distinguish UI assistance from the existing server-side validation.
- [ ] **List and object limits:** reflect `minItems`, `maxItems`, `uniqueItems`, `minProperties`, and `maxProperties` in editing controls. Explain limits, identify duplicate values, and appropriately constrain adding or removing entries.
- [ ] **Required matching items:** explain array requirements expressed through `contains`, `minContains`, and `maxContains`. Identify matching items and show how many matches are missing or excessive.
- [ ] **Standard annotations:** define and implement presentation behavior for `readOnly`, `writeOnly`, `deprecated`, and `examples`. Integrate with existing ownership rules; annotations alone are not validation or access-control enforcement.
- [ ] **Format-specific assistance:** provide appropriate hints or controls for formats such as `email`, `uri`, `time`, `duration`, and `uuid`, beyond the existing date/date-time presentation. Keep format validation separate from widget selection.
- [ ] **Encoded and structured string content:** support appropriate editing, decoding, and previews using `contentEncoding`, `contentMediaType`, and `contentSchema`. Define supported content types and explicit content-validation behavior; these keywords do not automatically require decoding or validation of embedded content.
- [ ] **Negative and forbidden schemas:** clearly explain restrictions expressed through `not` and boolean `false` schemas. Avoid offering forbidden properties or values as ordinary valid choices, while keeping existing invalid data visible and repairable.
- [ ] **Complete schema-reference resolution:** extend beyond local JSON Pointer references to registered external schema resources, relative references and `$id` scope, named `$anchor` references, and `$dynamicRef`/`$dynamicAnchor`. Use consistent resolution for validation and form rendering.
- [ ] **Unevaluated properties and items:** account for `unevaluatedProperties` and `unevaluatedItems` after other applicable schemas have been evaluated. Integrate this with composition so field and item controls reflect the combined schema rather than one branch in isolation.

Suggested priority among these additional gaps: constraint-aware controls, standard annotations, and complete schema-reference resolution. Follow up with keyword-by-keyword conformance tests; this checklist is based on a code/specification review, not a complete conformance audit.

Internationalization convention: keep translated interface labels in UI configuration and multilingual content in the data schema. JSON Schema has no standard internationalization keyword; a language-specific widget would be an optional presentation choice.
