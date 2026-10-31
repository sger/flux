package main

import (
	"fmt"
	"math"
)

const (
	solarMass   = 4.0 * math.Pi * math.Pi
	daysPerYear = 365.24
	dt          = 0.01
	steps       = 50000
)

type Body struct {
	x, y, z, vx, vy, vz, mass float64
}

func planet(x, y, z, vx, vy, vz, mass float64) Body {
	return Body{x, y, z, vx * daysPerYear, vy * daysPerYear, vz * daysPerYear, mass * solarMass}
}

func initial() []Body {
	return []Body{
		{0, 0, 0, 0, 0, 0, solarMass},
		planet(4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
			1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
			9.54791938424326609e-04),
		planet(8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
			-2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
			2.85885980666130812e-04),
		planet(1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
			2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
			4.36624404335156298e-05),
		planet(1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
			2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
			5.15138902046611451e-05),
	}
}

func offsetMomentum(bodies []Body) {
	var px, py, pz float64
	for _, b := range bodies {
		px += float64(b.vx * b.mass)
		py += float64(b.vy * b.mass)
		pz += float64(b.vz * b.mass)
	}
	bodies[0].vx = -px / solarMass
	bodies[0].vy = -py / solarMass
	bodies[0].vz = -pz / solarMass
}

// The float64 conversions stop the compiler fusing a multiply and an add into
// one instruction, which rounds differently from every other port.
func energy(bodies []Body) float64 {
	e := 0.0
	for i := range bodies {
		b := &bodies[i]
		e += float64(0.5 * b.mass * (float64(b.vx*b.vx) + float64(b.vy*b.vy) + float64(b.vz*b.vz)))
		for j := i + 1; j < len(bodies); j++ {
			o := &bodies[j]
			dx := b.x - o.x
			dy := b.y - o.y
			dz := b.z - o.z
			e -= (b.mass * o.mass) / math.Sqrt(float64(dx*dx)+float64(dy*dy)+float64(dz*dz))
		}
	}
	return e
}

func advance(bodies []Body) {
	for i := range bodies {
		b := &bodies[i]
		for j := i + 1; j < len(bodies); j++ {
			o := &bodies[j]
			dx := b.x - o.x
			dy := b.y - o.y
			dz := b.z - o.z
			d2 := float64(dx*dx) + float64(dy*dy) + float64(dz*dz)
			mag := dt / float64(d2*math.Sqrt(d2))
			b.vx -= float64(dx * o.mass * mag)
			b.vy -= float64(dy * o.mass * mag)
			b.vz -= float64(dz * o.mass * mag)
			o.vx += float64(dx * b.mass * mag)
			o.vy += float64(dy * b.mass * mag)
			o.vz += float64(dz * b.mass * mag)
		}
	}
	for i := range bodies {
		b := &bodies[i]
		b.x += float64(dt * b.vx)
		b.y += float64(dt * b.vy)
		b.z += float64(dt * b.vz)
	}
}

func main() {
	bodies := initial()
	offsetMomentum(bodies)
	fmt.Println(int64(math.Round(energy(bodies) * 1e9)))
	for i := 0; i < steps; i++ {
		advance(bodies)
	}
	fmt.Println(int64(math.Round(energy(bodies) * 1e9)))
}
