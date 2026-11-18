//! Correctly configured empty repository state.

use std::sync::Mutex;

use super::*;
use crate::inventory::ActiveRecords;
use crate::lifecycle::IndexRevision;
use crate::lifecycle_catalog::FileCatalog;
use crate::repository::*;
use crate::source::SourceRegistry;

pub(crate) fn initialize_empty(
    root: PathBuf,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: &SourceRegistry,
) -> Result<Repository, String> {
    let configuration_identity =
        crate::configuration::configuration_hash(&definitions, options, sources)?;
    let build_workers = crate::build::resolve_build_workers(
        options.build_workers,
        std::thread::available_parallelism().ok(),
    )?;
    let files = Arc::new(FileInventory::new(root));
    let empty_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .thread_name(|index| format!("objekte-empty-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    let collection_count = definitions.len();
    let mut collections = Vec::with_capacity(collection_count);
    let mut pending_relations = Vec::with_capacity(collection_count);
    for (number, definition) in definitions.into_iter().enumerate() {
        if collections
            .iter()
            .any(|state: &CollectionState| state.definition.name == definition.name)
        {
            return Err(format!("duplicate collection: {}", definition.name));
        }
        let id = CollectionId(u32::try_from(number).map_err(|_| "too many collections")?);
        let source = sources.resolve(&definition.format)?;
        let generation =
            CollectionBuilder::new(id, definition.name.clone(), 1, 0).finish(&files)?;
        let postings = PostingCollection::new(&definition.schema);
        let inference = InferenceBuilder::for_collection(options.inference, collection_count)
            .map(|builder| builder.finish(&RoaringBitmap::new(), 0, &definition.schema))
            .transpose()?;
        let custom_postings: HashMap<_, _> = definition
            .schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| {
                (
                    key.clone(),
                    CustomPostingIndex {
                        index: PostingIndex::new(&definition.schema.fields[&key.0]),
                        complete: binding.operation.complete_coverage(),
                    },
                )
            })
            .collect();
        let raw =
            BigramContribution::new(options.bigram_alphabet).into_index_in_pool(&empty_pool)?;
        let fallback_text =
            BigramContribution::new(options.bigram_alphabet).into_index_in_pool(&empty_pool)?;
        let custom_text = definition
            .schema
            .custom
            .keys()
            .chain(
                definition
                    .schema
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                BigramContribution::new(options.bigram_alphabet)
                    .into_index_in_pool(&empty_pool)
                    .map(|index| {
                        (
                            canonical_custom_text_key(&definition.schema, &key.0, &key.1),
                            index,
                        )
                    })
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let logical_records = RoaringBitmap::new();
        let empty_array_records = RoaringBitmap::new();
        let sort_builder = SortBuilder::new(
            &definition.sort_fields,
            &definition.schema,
            &definition.sort_locale,
            definition.sort_options,
            &definition.sort_overrides,
        )?;
        let sorting = sort_builder.finish(&generation)?;
        let text_bindings = crate::index_catalog::TextBindingCatalog::new(&definition.schema);
        let index_family_bytes = crate::lifecycle::index_family_total(
            &raw,
            &fallback_text,
            &custom_text,
            &postings,
            &sorting,
        );
        pending_relations.push(RelationContribution::for_schema(&definition.schema));
        collections.push(CollectionState {
            repository_instance: 0,
            definition,
            source,
            generation,
            postings,
            inference,
            global_inference: None,
            custom_postings,
            raw,
            fallback_text,
            logical_records,
            empty_array_records,
            custom_text,
            text_bindings,
            sorting,
            index_family_bytes,
            id_lookup: HashMap::new(),
            identity_order: Vec::new(),
            stats: CollectionStats {
                file_count: 0,
                record_count: 0,
                total_file_bytes: 0,
            },
            relations: Vec::new(),
        });
    }
    for (source_index, contributions) in pending_relations.into_iter().enumerate() {
        let source = collections[source_index].generation.collection;
        let indexes = contributions
            .into_iter()
            .enumerate()
            .map(|(ordinal, contribution)| {
                contribution.resolve(
                    source,
                    u32::try_from(ordinal).map_err(|_| "too many relations")?,
                    &collections,
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        collections[source_index].relations = indexes;
    }
    crate::relation::validate_indexes(&collections)?;
    let workers = Repository::create_workers(options.query_workers)?;
    let catalog = FileCatalog::from_parts(Arc::clone(&files), Vec::new(), configuration_identity);
    let active = (0..collections.len())
        .map(|_| ActiveRecords::all(0))
        .collect::<Result<Vec<_>, _>>()?;
    let segments = vec![Arc::new(Vec::new()); collections.len()];
    let instance_id = next_repository_instance();
    for collection in &mut collections {
        collection.repository_instance = instance_id;
    }
    Ok(Repository {
        files,
        base_fingerprints: Vec::new(),
        collections,
        active,
        segments,
        catalog,
        revision: IndexRevision::INITIAL,
        sources: sources.clone(),
        build_parses: 0,
        build_failed_parses: 0,
        instance_id,
        options,
        startup: StartupStats {
            build_workers,
            ..StartupStats::default()
        },
        workers,
        verified: Mutex::new(crate::verified::VerifiedCache::new(
            options.verified_cache_bytes,
        )),
        inferred_orders: Mutex::new(crate::sorting::InferredOrderCache::new(
            crate::sorting::INFERRED_ORDER_CACHE_BYTES,
        )),
    })
}
