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


# add from registry
cook add raylib

# or add from a repo directly
cook add raylib https://github.com/raysan5/raylib.git

# build the project
cook 
# OR
cook build
# OR (release profile)
cook build -r

# clean build artifacts
cook clean

# run the project
cook run
```

## cookbook
the [cookbook](https://github.com/Helix128/cookbook) is cook's package registry, it is a simple git repository that contains a collection of recipes. each "recipe" is a `.toml` file that describes the package and where to download it from.

## disclaimer
this project is just an experiment that i made to kill some time, it might work or it might not in some cases, and the code is sloppy at times, i'll probably rewrite it from scratch in a while, but for now, feel free to give it a try
