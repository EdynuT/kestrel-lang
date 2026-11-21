interface Forma {
    proc area(&self) -> real;
    proc nome(&self) -> text;
}

@impl(Forma)
class Circulo {
    raio: real;

    public proc novo(raio: real) -> Circulo {
        return Circulo { raio: raio };
    }

    @override
    public proc area(&self) -> real {
        return 3.14 * self.raio ** 2;
    }

    @override
    public proc nome(&self) -> text {
        return "círculo";
    }
}

 @impl(Forma)
 class Poligono {
     largura: real;
     altura: real;

     public proc novo(largura: real, altura: real) -> Poligono {
         return Poligono { largura: largura, altura: altura };
     }

     @override
     public proc area(&self) -> real {
         return self.largura * self.altura;
     }

     @override
     public proc nome(&self) -> text {
         return "polígono";
     }
 }

 proc mostrar(f: &Forma) {
    printf("{f.nome()}: {f.area()}\n");
}

proc main() {
    f: Forma = Circulo.novo(1.0);
    formas[]: Forma = {give f, Poligono.novo(3.0, 4.0)};
    append(formas, Circulo.novo(2.0));
    for i: int = 0; i < len(formas); i++ {
        mostrar(&formas[i]);   // cada uma chama o seu próprio `area`
    }
    burn formas;
}
