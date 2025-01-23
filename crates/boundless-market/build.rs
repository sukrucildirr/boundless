// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{env, fs, fs::File, io::Write, path::Path};

// Contracts to copy to the artificats folder for. If the contract is a directory, all .sol files in the directory.
const CONTRACTS_TO_COPY: [&str; 3] = ["IBoundlessMarket.sol", "IHitPoints.sol", "types"];

// Contracts to exclude from generating types for automatically.
const EXCLUDE_CONTRACTS: [&str; 2] = [
    // Exclude Account as the type of `requestFlagsExtended` is not supported for type generation.
    "Account.sol",
    "IHitPoints.sol",
];

// Contracts to copy bytecode for. Used for deploying contracts in tests.
const ARTIFACT_TARGET_CONTRACTS: [&str; 5] =
    ["BoundlessMarket", "HitPoints", "RiscZeroMockVerifier", "RiscZeroSetVerifier", "ERC1967Proxy"];

// Output filename for the generated types. The file is placed in the build directory.
const BOUNDLESS_MARKET_RS: &str = "boundless_market_generated.rs";

fn insert_derives(contents: &mut String, find_str: &str, insert_str: &str) {
    let mut cur_pos = 0;
    while let Some(struct_pos) =
        contents.match_indices(find_str).find_map(|(i, _)| (i >= cur_pos).then_some(i))
    {
        // println!("cargo:warning={struct_pos}");
        contents.insert_str(struct_pos, insert_str);
        cur_pos = struct_pos + insert_str.len() + find_str.len();
    }
}

// TODO: This is a bit fragile (e.g. it breaks if there is an unmatched brace in a comment).
// Using alloy's `syn-solidity` would be the robust way of doing this.
// (It may also be over-engineering, as we'd like to deprecate this whole script)
fn find_matching_brace(contents: &str) -> Option<usize> {
    let mut stack = Vec::new();
    for (i, c) in contents.char_indices() {
        match c {
            '{' => stack.push(c),
            '}' => {
                stack.pop();
                if stack.is_empty() {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

// NOTE: if alloy ever fixes https://github.com/alloy-rs/core/issues/688 this function
// can be deleted and we should be able to just use the alloy::sol! macro
// Note, we also remove libraries from each file, as some of the libraries reference
// the `Account` struct, which we do not support (see EXCLUDE_CONTRACTS at top).
fn rewrite_solidity_interface_files() {
    println!("cargo::rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let sol_iface_dir = Path::new(&manifest_dir).join("src").join("contracts").join("artifacts");
    println!("cargo::rerun-if-changed={}", sol_iface_dir.to_string_lossy());
    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    let mut combined_sol_contents = String::new();

    for entry in fs::read_dir(sol_iface_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("sol") {
            // Skip if the file is in EXCLUDE_CONTRACTS
            if EXCLUDE_CONTRACTS.contains(&path.file_name().unwrap().to_str().unwrap()) {
                continue;
            }

            let mut sol_contents = fs::read_to_string(&path).unwrap();

            // Remove libraries from each file.
            while let Some(start) = sol_contents.find("library ") {
                if let Some(end) = find_matching_brace(&sol_contents[start..]) {
                    sol_contents.replace_range(start..start + end + 1, "");
                } else {
                    // print the file name and panic if we can't find the matching brace
                    panic!("Unmatched brace in library {:?}", entry);
                }
            }

            // skip the sol(rpc) insert if building for the zkvm
            if target_os != "zkvm" {
                if let Some(iface_pos) = sol_contents.find("interface ") {
                    sol_contents.insert_str(iface_pos, "#[sol(rpc)]\n");
                }
            }

            insert_derives(&mut sol_contents, "\nstruct ", "\n#[derive(Deserialize, Serialize)]");
            insert_derives(&mut sol_contents, "\nenum ", "\n#[derive(Deserialize, Serialize)]");

            combined_sol_contents.push_str(&sol_contents);
        }
    }

    let mut alloy_import = "alloy_sol_types";
    if target_os != "zkvm" {
        alloy_import = "alloy";
    }

    println!("cargo::rerun-if-env-changed=OUT_DIR");
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join(BOUNDLESS_MARKET_RS);
    fs::write(
        dest_path,
        format!(
            "#[allow(missing_docs)]
        pub mod boundless_market_contract {{
            use serde::{{Deserialize, Serialize}};
            {alloy_import}::sol! {{
            #![sol(all_derives)]
            {combined_sol_contents}
}}
}}
        "
        ),
    )
    .unwrap();
}

fn copy_interfaces_and_types() {
    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dest_path = Path::new(&manifest_dir).join("src/contracts/artifacts");
    fs::create_dir_all(&dest_path).unwrap();

    let src_path =
        Path::new(&manifest_dir).parent().unwrap().parent().unwrap().join("contracts").join("src");

    let contracts_to_copy: Vec<String> = CONTRACTS_TO_COPY
        .iter()
        .flat_map(|contract| {
            if contract.ends_with(".sol") {
                vec![contract.to_string()]
            } else {
                let dir_path = src_path.join(contract);
                fs::read_dir(dir_path)
                    .unwrap()
                    .filter_map(|entry| {
                        let path = entry.unwrap().path();
                        if path.extension().and_then(|s| s.to_str()) == Some("sol") {
                            Some(format!(
                                "{}/{}",
                                contract,
                                path.file_name().unwrap().to_str().unwrap()
                            ))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
        })
        .collect();

    println!("contracts_to_copy: {:?}", contracts_to_copy);

    for contract in contracts_to_copy {
        let source_path = src_path.join(&contract);
        // Tell cargo to rerun if this contract changes
        println!("cargo:rerun-if-changed={}", source_path.display());

        if source_path.exists() {
            // Copy the file to the destination without directory prefixes
            let dest_file_name = contract.split('/').last().unwrap();
            let dest_file_path = dest_path.join(dest_file_name);
            println!("Copying {:?} to {:?}", source_path, dest_file_path);
            std::fs::copy(&source_path, dest_file_path).unwrap();
        }
    }
}

fn copy_artifacts() {
    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let dest_path = Path::new(&manifest_dir).join("src/contracts/artifacts");
    fs::create_dir_all(&dest_path).unwrap();

    let src_path =
        Path::new(&manifest_dir).parent().unwrap().parent().unwrap().join("contracts").join("out");

    for contract in ARTIFACT_TARGET_CONTRACTS {
        let source_path = src_path.join(format!("{contract}.sol/{contract}.json"));
        // Tell cargo to rerun if this contract changes
        println!("cargo:rerun-if-changed={}", source_path.display());

        if source_path.exists() {
            // Read and parse the JSON file
            let json_content = fs::read_to_string(&source_path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&json_content).unwrap();

            // Extract the bytecode, removing "0x" prefix if present
            let bytecode = json["bytecode"]["object"]
                .as_str()
                .ok_or(format!(
                    "failed to extract bytecode from {}",
                    source_path.as_os_str().to_string_lossy()
                ))
                .unwrap()
                .trim_start_matches("0x");

            // Write to new file with .hex extension
            let dest_file = dest_path.join(format!("{contract}.hex"));
            let mut file = File::create(dest_file).unwrap();
            file.write_all(bytecode.as_bytes()).unwrap();
        }
    }
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    copy_interfaces_and_types();
    rewrite_solidity_interface_files();

    println!("cargo::rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    if target_os != "zkvm" {
        copy_artifacts();
    }
}
