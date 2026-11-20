interface Forma {
    proc area(lent self) -> real;
    proc nome(lent self) -> text;
}

@impl(Forma)
class Circulo {
    raio: real;

    public proc novo(raio: real) -> Circulo {
        return Circulo { raio: raio };
    }

    @override
    public proc area(lent self) -> real {
        return 3.14 * self.raio ** 2;
    }

    @override
    public proc nome(lent self) -> text {
        return "círculo";
    }
}

proc mostrar(lent f: Forma) {
    printf("{f.nome()}: {f.area()}\n");
}

proc main() {
    c: Circulo = Circulo.novo(2.0);
    mostrar(lend c);
    burn c;
}
