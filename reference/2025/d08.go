package main

import (
	"bufio"
	"fmt"
	"os"
	"slices"
	"strconv"
	"strings"
)

type p3d struct{ x, y, z int64 }
type edge struct{ u, v int }

func (p p3d) distSq(q p3d) int64 {
	dx, dy, dz := p.x-q.x, p.y-q.y, p.z-q.z
	return dx*dx + dy*dy + dz*dz
}

type unionFind struct {
	parents, sizes []int
	count          int
}

func newUnionFind(n int) *unionFind {
	parents, sizes := make([]int, n), make([]int, n)
	for i := range parents {
		parents[i] = i
		sizes[i] = 1
	}
	return &unionFind{parents, sizes, n}
}

func (u *unionFind) find(i int) int {
	if u.parents[i] != i {
		u.parents[i] = u.find(u.parents[i])
	}
	return u.parents[i]
}

func (u *unionFind) union(i, j int) bool {
	rooti, rootj := u.find(i), u.find(j)
	if rooti == rootj {
		return false
	}
	if u.sizes[rooti] < u.sizes[rootj] {
		rooti, rootj = rootj, rooti
	}
	u.parents[rootj], u.sizes[rooti] = rooti, u.sizes[rooti]+u.sizes[rootj]
	u.count -= 1
	return true
}

func main() {
	f, _ := os.Open("y25d08.txt")
	defer f.Close()

	var pts []p3d
	for s := bufio.NewScanner(f); s.Scan(); {
		if strings.TrimSpace(s.Text()) == "" {
			continue
		}
		p := strings.Split(s.Text(), ",")
		x, _ := strconv.ParseInt(p[0], 10, 64)
		y, _ := strconv.ParseInt(p[1], 10, 64)
		z, _ := strconv.ParseInt(p[2], 10, 64)
		pts = append(pts, p3d{x, y, z})
	}

	var edges []edge
	for i := range pts {
		for j := i + 1; j < len(pts); j++ {
			edges = append(edges, edge{i, j})
		}
	}
	slices.SortFunc(edges, func(a, b edge) int {
		return int(pts[a.u].distSq(pts[a.v]) - pts[b.u].distSq(pts[b.v]))
	})

	limit := 1000
	if v := os.Getenv("PAIRS"); v != "" {
		limit, _ = strconv.Atoi(v)
	}
	uf1 := newUnionFind(len(pts))
	for _, e := range edges[:min(limit, len(edges))] {
		uf1.union(e.u, e.v)
	}
	sizes := make(map[int]int)
	for i := range pts {
		sizes[uf1.find(i)] += 1
	}
	var groups []int
	for _, v := range sizes {
		groups = append(groups, v)
	}
	slices.SortFunc(groups, func(a, b int) int { return b - a })
	fmt.Println("Result1:", groups[0]*groups[1]*groups[2])

	uf2 := newUnionFind(len(pts))
	for _, e := range edges {
		if uf2.union(e.u, e.v) && uf2.count == 1 {
			fmt.Println("Result2:", pts[e.u].x*pts[e.v].x)
			break
		}
	}
}
