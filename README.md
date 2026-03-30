# cook - toy C/C++ package manager(?)

cook is a simple c/c++ package manager that can build and manage dependencies for C/C++ projects.
it works above a true build system (currently cmake is supported) and provides a simple interface to manage dependencies and build projects.

its VERY inspired by rust's [cargo](https://github.com/rust-lang/cargo)

## example

```bash
# create a new project
cook new my_project

# add a dependency
cd my_project
cook add raylib

# build the project
cook 
# OR
cook build
# OR (release profile)
cook build -r

# run the project
cook run
```

## cookbook
the [cookbook](https://github.com/Helix128/cookbook) is cook's package registry, it is a simple git repository that contains a collection of recipes. each recipe is a directory that contains a `.toml` file that describes the package and where to download it from.