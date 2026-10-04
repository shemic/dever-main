package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"go/build"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type selectedPackage struct {
	Name    string              `json:"name"`
	Files   []string            `json:"files"`
	Imports []string            `json:"imports"`
	Embed   map[string][]string `json:"embed"`
}

func main() {
	if len(os.Args) != 4 {
		fail(errors.New("usage: analyze <goos> <goarch> <directory>"))
	}
	ctx := build.Default
	ctx.GOOS, ctx.GOARCH = os.Args[1], os.Args[2]
	ctx.CgoEnabled = false
	ctx.BuildTags = nil
	pkg, err := ctx.ImportDir(os.Args[3], 0)
	if err != nil {
		fail(err)
	}
	if len(pkg.CgoFiles)+len(pkg.SFiles)+len(pkg.CFiles)+len(pkg.CXXFiles)+len(pkg.SwigFiles)+len(pkg.SwigCXXFiles) != 0 {
		fail(errors.New("managed Go Worker does not support cgo, assembly, or native source"))
	}
	if len(pkg.GoFiles) == 0 {
		fail(errors.New("no Go source selected for target"))
	}
	embed := make(map[string][]string, len(pkg.EmbedPatterns))
	for _, rawPattern := range pkg.EmbedPatterns {
		includeHidden := strings.HasPrefix(rawPattern, "all:")
		pattern := strings.TrimPrefix(rawPattern, "all:")
		matches, err := filepath.Glob(filepath.Join(os.Args[3], filepath.FromSlash(pattern)))
		if err != nil || len(matches) == 0 {
			fail(fmt.Errorf("Go embed pattern %q has no matches", rawPattern))
		}
		for _, match := range matches {
			err = filepath.WalkDir(match, func(path string, entry os.DirEntry, walkErr error) error {
				if walkErr != nil {
					return walkErr
				}
				if entry.Type()&os.ModeSymlink != 0 {
					return errors.New("Go embed contains a symbolic link")
				}
				if !includeHidden && path != match && (strings.HasPrefix(entry.Name(), ".") || strings.HasPrefix(entry.Name(), "_")) {
					if entry.IsDir() {
						return filepath.SkipDir
					}
					return nil
				}
				if entry.IsDir() {
					return nil
				}
				relative, err := filepath.Rel(os.Args[3], path)
				if err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
					return errors.New("Go embed escapes package directory")
				}
				embed[rawPattern] = append(embed[rawPattern], filepath.ToSlash(relative))
				return nil
			})
			if err != nil {
				fail(err)
			}
		}
		if len(embed[rawPattern]) == 0 {
			fail(fmt.Errorf("Go embed pattern %q has no embeddable files", rawPattern))
		}
		sort.Strings(embed[rawPattern])
	}
	result := selectedPackage{Name: pkg.Name, Files: pkg.GoFiles, Imports: pkg.Imports, Embed: embed}
	if err := json.NewEncoder(os.Stdout).Encode(result); err != nil {
		fail(err)
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, err)
	os.Exit(1)
}
