use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use soil_abi::{CONTRACTS, DEPENDENCIES};
use wit_parser::Resolve;

#[test]
fn every_capability_contract_parses_with_its_reviewed_package_name() {
    let mut resolve = Resolve::default();
    let mut packages = BTreeMap::new();

    for dependency in DEPENDENCIES {
        resolve
            .push_str(dependency.file, dependency.source)
            .unwrap_or_else(|error| {
                panic!("{} must be valid WIT: {error}", dependency.file);
            });
    }

    for contract in CONTRACTS {
        let id = resolve
            .push_str(contract.file, contract.source)
            .unwrap_or_else(|error| {
                panic!("{} must be valid WIT: {error}", contract.file);
            });
        packages.insert(contract.package, id);
    }

    for contract in CONTRACTS {
        let id = packages[contract.package];
        let package = &resolve.packages[id];
        assert_eq!(
            package.name.to_string(),
            contract.package,
            "{} changed its package name",
            contract.file
        );

        for (interface_name, functions) in contract.interfaces {
            let interface_id = *package.interfaces.get(*interface_name).unwrap_or_else(|| {
                panic!("{} is missing interface `{interface_name}`", contract.file)
            });
            let interface = &resolve.interfaces[interface_id];
            for function in *functions {
                assert!(
                    interface.functions.contains_key(*function),
                    "{}::{interface_name} is missing `{function}`",
                    contract.file
                );
            }
        }
    }
}

#[test]
fn capability_packages_are_versioned_and_namespaced() {
    for contract in CONTRACTS {
        assert!(
            contract.package.starts_with("soil:")
                || contract.package.starts_with("wasi:")
                || contract.package.starts_with("fungos:"),
            "{} must stay in the soil, wasi or fungos namespace",
            contract.file
        );
        assert!(
            contract.package.contains('@'),
            "{} must carry a version",
            contract.file
        );
    }
}

#[test]
fn package_names_are_unique() {
    for (index, contract) in CONTRACTS.iter().enumerate() {
        for other in &CONTRACTS[index + 1..] {
            assert_ne!(
                contract.package, other.package,
                "{} and {} expose the same package",
                contract.file, other.file
            );
        }
    }
}

/// The reviewed list is the point of this crate, but it must not silently fall
/// behind the directory it claims to cover. A new `wit/*.wit` file has to be
/// added here with a reviewed package name; a stale file reference fails too.
#[test]
fn every_wit_file_is_a_reviewed_contract() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let wit_dir = root.join("wit");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&wit_dir).unwrap_or_else(|error| {
        panic!("read {}: {error}", wit_dir.display());
    }) {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) == Some("wit") {
            files.push(format!(
                "wit/{}",
                path.file_name().unwrap().to_string_lossy()
            ));
        }
    }
    files.sort();

    let reviewed = CONTRACTS
        .iter()
        .map(|contract| contract.file)
        .collect::<BTreeSet<_>>();
    let missing = files
        .iter()
        .filter(|file| !reviewed.contains(file.as_str()))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "unreviewed WIT packages under wit/: {missing:?}"
    );

    for file in reviewed {
        assert!(
            root.join(file).is_file(),
            "{file} is reviewed but does not exist"
        );
    }
    for dependency in DEPENDENCIES {
        assert!(
            root.join(dependency.file).is_file(),
            "{} is a vendored dependency but does not exist",
            dependency.file
        );
    }
}
