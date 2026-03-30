pub fn generate_cmake(name: &str, cpp_standard: &str, sources: &[String], dependencies: &[String]) -> String {
    let sources_joined = sources.join("\n    ");

    let includes_block = dependencies
        .iter()
        .map(|dep| format!("add_subdirectory(.cook/deps/{})", dep))
        .collect::<Vec<String>>()
        .join("\n");

    let links_block = dependencies.join(" ");

    format!(
        r#"cmake_minimum_required(VERSION 3.15)
project({name} CXX)

set(CMAKE_CXX_STANDARD {std})
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

{includes_block}

add_executable({name}
    {sources}
)

if(NOT "{links_block}" STREQUAL "")
    target_link_libraries({name} PRIVATE {links_block})
endif()
"#,
        name = name,
        std = cpp_standard,
        sources = sources_joined,
        includes_block = includes_block,
        links_block = links_block
    )
}